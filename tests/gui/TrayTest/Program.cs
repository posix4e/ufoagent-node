// Headful GUI test: launch `ufoagent tray`, find the 🛸 tray icon, open its menu, invoke an item.
//
// First cut is diagnostic-heavy: it dumps the taskbar / notification-area UIA subtree so the CI
// log reveals the real structure on the runner (Windows Server 2025), since the system tray is
// notoriously layout-specific (Win11 overflow flyout, etc.). Exit codes: 0 pass, 2 icon not
// found, 3 menu item not found, 1 launch/automation error.

using System;
using System.Diagnostics;
using System.Linq;
using System.Threading;
using FlaUI.Core;
using FlaUI.Core.AutomationElements;
using FlaUI.Core.Definitions;
using FlaUI.UIA3;

internal static class Program
{
    private static int Main(string[] args)
    {
        var exe = args.Length > 0 ? args[0] : @"target\release\ufoagent.exe";
        Console.WriteLine($"[tray-test] launching: {exe} tray");
        Application app;
        try { app = Application.Launch(exe, "tray"); }
        catch (Exception e) { Console.Error.WriteLine($"[tray-test] launch failed: {e.Message}"); return 1; }

        try
        {
            using var automation = new UIA3Automation();
            var desktop = automation.GetDesktop();
            Thread.Sleep(5000); // give the tray icon time to register

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

            // Match the tray-icon BUTTON named ~UFOAgent — NOT a Window (e.g. the 'UFOAgent setup'
            // installer/bootstrap console, which would otherwise match and have no Repair menu).
            var icon = FindButton(desktop, "UFOAgent");
            if (icon == null) { Console.Error.WriteLine("[tray-test] FAIL: UFOAgent tray icon (button) not found"); return 2; }
            Console.WriteLine($"[tray-test] found tray icon: '{Name(icon)}' [{Type(icon)}]");

            try { icon.RightClick(); } catch (Exception e) { Console.Error.WriteLine($"[tray-test] right-click failed: {e.Message}"); return 1; }
            Thread.Sleep(1500);

            var repair = desktop.FindAllDescendants()
                .FirstOrDefault(e => Type(e) == ControlType.MenuItem.ToString()
                                     && Name(e).IndexOf("Repair", StringComparison.OrdinalIgnoreCase) >= 0);
            if (repair == null) { Console.Error.WriteLine("[tray-test] FAIL: 'Repair' menu item not found after right-click"); return 3; }

            Console.WriteLine($"[tray-test] invoking menu item: '{Name(repair)}'");
            try { repair.Click(); } catch (Exception e) { Console.Error.WriteLine($"[tray-test] invoke failed: {e.Message}"); return 1; }
            Thread.Sleep(1500);

            Console.WriteLine("[tray-test] PASS: opened the tray menu and invoked Repair");
            return 0;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[tray-test] automation error: {e}");
            return 1;
        }
        finally
        {
            try { foreach (var p in Process.GetProcessesByName("ufoagent")) p.Kill(); } catch { }
        }
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
