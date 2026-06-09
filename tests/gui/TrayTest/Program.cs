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
        try { Directory.CreateDirectory(_shots); } catch { }

        Console.WriteLine($"[tray-test] launching: {exe} tray (screenshots -> {_shots})");
        Application app;
        try { app = Application.Launch(exe, "tray"); }
        catch (Exception e) { Console.Error.WriteLine($"[tray-test] launch failed: {e.Message}"); return 1; }

        try
        {
            using var automation = new UIA3Automation();
            var desktop = automation.GetDesktop();
            Thread.Sleep(5000); // give the tray icon time to register
            Shot("launched");

            DumpTaskbar(desktop);

            // The tray icon usually sits in the overflow ("Show Hidden Icons") on Server 2025 — open it.
            var chevron = FindButton(desktop, "Show Hidden Icons") ?? FindButton(desktop, "hidden icons");
            if (chevron != null)
            {
                Console.WriteLine($"[tray-test] opening overflow via '{chevron.Name}'");
                try { chevron.Click(); } catch { }
                Thread.Sleep(1500);
            }
            else { Console.WriteLine("[tray-test] no overflow chevron found; trying the visible area"); }
            Shot("overflow");

            // Match the tray-icon BUTTON named ~UFOAgent — NOT a Window (e.g. the 'UFOAgent setup'
            // installer/bootstrap console, which would otherwise match and have no Repair menu).
            var icon = FindButton(desktop, "UFOAgent");
            if (icon == null) { Console.Error.WriteLine("[tray-test] FAIL: UFOAgent tray icon (button) not found"); Shot("FAIL-no-icon"); return 2; }
            Console.WriteLine($"[tray-test] found tray icon: '{Name(icon)}' [{Type(icon)}]");
            Shot("icon-found");

            try { icon.RightClick(); } catch (Exception e) { Console.Error.WriteLine($"[tray-test] right-click failed: {e.Message}"); Shot("FAIL-rightclick"); return 1; }
            Thread.Sleep(1500);
            Shot("menu-open");

            var repair = desktop.FindAllDescendants()
                .FirstOrDefault(e => Type(e) == ControlType.MenuItem.ToString()
                                     && Name(e).IndexOf("Repair", StringComparison.OrdinalIgnoreCase) >= 0);
            if (repair == null) { Console.Error.WriteLine("[tray-test] FAIL: 'Repair' menu item not found after right-click"); Shot("FAIL-no-menu"); return 3; }

            Console.WriteLine($"[tray-test] invoking menu item: '{Name(repair)}'");
            try { repair.Click(); } catch (Exception e) { Console.Error.WriteLine($"[tray-test] invoke failed: {e.Message}"); Shot("FAIL-invoke"); return 1; }
            Thread.Sleep(1500);
            Shot("after-repair");

            Console.WriteLine("[tray-test] PASS: opened the tray menu and invoked Repair");
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
