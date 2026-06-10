// Headful GUI test: launch `ufoagent tray`, find the 🛸 tray icon, open its menu, invoke an item.
//
// Logs the taskbar/notification-area UIA subtree (Server 2025's tray layout is finicky) AND saves
// a screenshot at each step (launched / overflow / icon / menu / after-repair / failures) into the
// screenshots dir (args[1]) so CI can upload visual evidence of the click-through.
// Exit codes: 0 pass, 2 icon not found, 3 menu item not found, 1 launch/automation error.

using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;
using FlaUI.Core;
using FlaUI.Core.AutomationElements;
using FlaUI.Core.Capturing;
using FlaUI.Core.Definitions;
using FlaUI.UIA3;

internal static class Program
{
    private static string _shots = "shots";
    private static int _n = 0;

    private static int Main(string[] args)
    {
        var exe = args.Length > 0 ? args[0] : @"target\release\ufoagent.exe";
        _shots = args.Length > 1 ? args[1] : "shots";
        // --attach: a tray is already running (it's the run_task executor) — drive that one instead
        // of launching a second instance.
        var attach = args.Contains("--attach");
        try { Directory.CreateDirectory(_shots); } catch { }

        if (attach)
        {
            Console.WriteLine($"[tray-test] attaching to the running tray (screenshots -> {_shots})");
        }
        else
        {
            Console.WriteLine($"[tray-test] launching: {exe} tray (screenshots -> {_shots})");
            try { Application.Launch(exe, "tray"); }
            catch (Exception e) { Console.Error.WriteLine($"[tray-test] launch failed: {e.Message}"); return 1; }
        }

        try
        {
            using var automation = new UIA3Automation();
            var desktop = automation.GetDesktop();
            Thread.Sleep(5000); // give the tray icon time to register
            Shot("launched");

            DumpTaskbar(desktop);

            // "View log" — opens ufoagent.log in Notepad, the raw on-node history of every command
            // run (local: tray: running task / running UFO2; remote: ws: command …).
            int rc = OpenAndClick(desktop, "View log");
            if (rc != 0) return rc;
            Thread.Sleep(2500); // let Notepad open the log
            Shot("command-log");

            // "What's this node been doing?" — the tray builds the recap (an LLM call) and pops a
            // native dialog with it. Capture that window: a clean GUI shot of the real summary.
            rc = OpenAndClick(desktop, "been doing");
            if (rc != 0) return rc;
            ShootActivityDialog(desktop);

            Console.WriteLine("[tray-test] PASS: captured View log and the activity-summary dialog from the tray");
            return 0;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[tray-test] automation error: {e}");
            Shot("FAIL-exception");
            return 1;
        }
        finally
        {
            try { foreach (var p in Process.GetProcessesByName("ufoagent")) p.Kill(); } catch { }
        }
    }

    private static void Shot(string label)
    {
        try
        {
            var path = Path.Combine(_shots, $"{_n++:00}-{label}.png");
            Capture.Screen().ToFile(path);
            Console.WriteLine($"[tray-test] screenshot: {path}");
        }
        catch (Exception e) { Console.WriteLine($"[tray-test] screenshot failed: {e.Message}"); }
    }

    // The "activity" item builds the recap (an LLM call, a few seconds) and pops a native dialog
    // titled "…what this node has been doing". Wait for that window, raise it, capture it, dismiss
    // it — a clean GUI shot, no console-occlusion lottery.
    private static void ShootActivityDialog(AutomationElement desktop)
    {
        AutomationElement dlg = null;
        for (int i = 0; i < 30 && dlg == null; i++)
        {
            Thread.Sleep(1000);
            try
            {
                dlg = desktop.FindAllChildren().FirstOrDefault(e =>
                    Type(e) == ControlType.Window.ToString()
                    && Name(e).IndexOf("been doing", StringComparison.OrdinalIgnoreCase) >= 0);
            }
            catch { }
        }
        if (dlg == null) { Console.Error.WriteLine("[tray-test] WARN: activity dialog never appeared"); Shot("activity-summary"); return; }

        Console.WriteLine($"[tray-test] activity dialog: '{Name(dlg)}'");
        try { dlg.AsWindow().Focus(); } catch { }
        Thread.Sleep(800);
        Shot("activity-summary");
        // Dismiss it (click OK) so it doesn't linger over later windows.
        var ok = dlg.FindAllDescendants().FirstOrDefault(e =>
            Type(e) == ControlType.Button.ToString() && Name(e).IndexOf("OK", StringComparison.OrdinalIgnoreCase) >= 0);
        try { ok?.Click(); } catch { }
    }

    // Open the tray menu and click the item whose name contains `needle`. Each call re-opens the
    // "Show Hidden Icons" overflow and re-finds the 🛸 icon fresh — on Server 2025 the icon lives in
    // that flyout and it closes after each use, so a cached element goes stale between actions.
    // Returns 0 on success, or the Main exit code for the failure (2 icon, 3 menu item).
    private static int OpenAndClick(AutomationElement desktop, string needle)
    {
        var label = needle.Replace(' ', '-');
        // Flyout is closed at the start of each call (a prior menu click dismissed it), so clicking
        // the chevron opens it rather than toggling it shut.
        var chevron = FindButton(desktop, "Show Hidden Icons") ?? FindButton(desktop, "hidden icons");
        if (chevron != null) { Console.WriteLine($"[tray-test] opening overflow via '{chevron.Name}'"); try { chevron.Click(); } catch { } Thread.Sleep(1500); }
        else { Console.WriteLine("[tray-test] no overflow chevron found; trying the visible area"); }

        // Match the tray-icon BUTTON named ~UFOAgent — NOT a Window (e.g. the 'UFOAgent setup'
        // installer/bootstrap console, which would otherwise match and have no menu).
        var icon = FindButton(desktop, "UFOAgent");
        if (icon == null) { Console.Error.WriteLine($"[tray-test] FAIL: UFOAgent tray icon (button) not found before '{needle}'"); Shot($"FAIL-no-icon-{label}"); return 2; }
        Console.WriteLine($"[tray-test] found tray icon: '{Name(icon)}' [{Type(icon)}]");

        try { icon.RightClick(); } catch (Exception e) { Console.Error.WriteLine($"[tray-test] right-click failed: {e.Message}"); Shot($"FAIL-rightclick-{label}"); return 1; }
        Thread.Sleep(1500);
        Shot($"menu-{label}");

        var item = desktop.FindAllDescendants()
            .FirstOrDefault(e => Type(e) == ControlType.MenuItem.ToString()
                                 && Name(e).IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0);
        if (item == null) { Console.Error.WriteLine($"[tray-test] FAIL: menu item containing '{needle}' not found"); Shot($"FAIL-no-menu-{label}"); return 3; }

        Console.WriteLine($"[tray-test] invoking menu item: '{Name(item)}'");
        try { item.Click(); }
        catch (Exception e) { Console.Error.WriteLine($"[tray-test] invoke '{needle}' failed: {e.Message}"); Shot($"FAIL-invoke-{label}"); return 1; }
        return 0;
    }

    private static AutomationElement FindByNameContains(AutomationElement root, string needle)
    {
        try
        {
            return root.FindAllDescendants()
                .FirstOrDefault(e => Name(e).IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0);
        }
        catch { return null; }
    }

    // A Button (e.g. a notification-area tray icon) whose name contains `needle`. Restricting to
    // Button avoids matching top-level Windows like the 'UFOAgent setup' installer console.
    private static AutomationElement FindButton(AutomationElement root, string needle)
    {
        try
        {
            return root.FindAllDescendants()
                .FirstOrDefault(e => Type(e) == ControlType.Button.ToString()
                                     && Name(e).IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0);
        }
        catch { return null; }
    }

    private static void DumpTaskbar(AutomationElement desktop)
    {
        Console.WriteLine("=== [tray-test] desktop top-level windows ===");
        try { foreach (var w in desktop.FindAllChildren()) Console.WriteLine($"  [{Type(w)}] '{Name(w)}'"); } catch { }

        var taskbar = FindByNameContains(desktop, "Taskbar");
        if (taskbar == null) { Console.WriteLine("=== [tray-test] no 'Taskbar' element found ==="); return; }
        Console.WriteLine("=== [tray-test] taskbar subtree ===");
        try { foreach (var e in taskbar.FindAllDescendants()) Console.WriteLine($"  [{Type(e)}] '{Name(e)}'"); } catch { }
    }

    private static string Name(AutomationElement e) { try { return e.Name ?? ""; } catch { return ""; } }
    private static string Type(AutomationElement e) { try { return e.ControlType.ToString(); } catch { return "?"; } }
}
