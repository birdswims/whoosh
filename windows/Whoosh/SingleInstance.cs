using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Whoosh;

internal static class SingleInstance
{
    static Mutex? _mutex;

    public static void SetAppId()
    {
        // The id is part of the window identity Windows uses to restore placement.
        SetCurrentProcessExplicitAppUserModelID("Whoosh.Desktop.1");
    }

    public static bool Acquire()
    {
        _mutex = new Mutex(false, @"Local\Whoosh.Desktop");
        try
        {
            if (_mutex.WaitOne(0))
            {
                return true;
            }
        }
        catch (AbandonedMutexException)
        {
            return true;
        }

        return false;
    }

    /// The calling process still has the user's foreground right. A hidden
    /// Whoosh window may not have a main handle until it is shown, so retry.
    public static void BringToFront()
    {
        for (var attempt = 0; attempt < 25; attempt++)
        {
            if (ActivateExisting())
            {
                return;
            }

            Thread.Sleep(100);
        }
    }

    public static bool ActivateExisting()
    {
        var current = Process.GetCurrentProcess().Id;
        foreach (var process in Process.GetProcessesByName("Whoosh"))
        {
            if (process.Id == current || process.MainWindowHandle == IntPtr.Zero)
            {
                continue;
            }

            ShowWindow(process.MainWindowHandle, 9);
            SetForegroundWindow(process.MainWindowHandle);
            return true;
        }

        return false;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern int SetCurrentProcessExplicitAppUserModelID(string appId);

    [DllImport("user32.dll")]
    static extern bool SetForegroundWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    static extern bool ShowWindow(IntPtr hwnd, int command);
}
