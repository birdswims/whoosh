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

        ActivateExisting();
        return false;
    }

    static void ActivateExisting()
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
            return;
        }
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern int SetCurrentProcessExplicitAppUserModelID(string appId);

    [DllImport("user32.dll")]
    static extern bool SetForegroundWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    static extern bool ShowWindow(IntPtr hwnd, int command);
}
