using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;

namespace Whoosh;

internal static class WindowChrome
{
    const int GwlpWndProc = -4;
    const uint WmGetMinMaxInfo = 0x0024;
    const uint WmDpiChanged = 0x02E0;

    static readonly WndProc Proc = Hook;
    static IntPtr _previous;
    static int _logicalWidth = 900;
    static int _logicalHeight = 640;
    static int _minWidth;
    static int _minHeight;

    public static void KeepAtLeast(Window window, int width, int height)
    {
        _logicalWidth = width;
        _logicalHeight = height;
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(window);
        var dpi = GetDpiForWindow(hwnd);
        if (dpi == 0)
        {
            dpi = 96;
        }

        ApplyDpi((int)dpi);
        var next = Marshal.GetFunctionPointerForDelegate(Proc);
        var previous = SetWindowLongPtr(hwnd, GwlpWndProc, next);
        if (previous != IntPtr.Zero && previous != next)
        {
            _previous = previous;
        }
    }

    public static void Place(Window window, int width, int height)
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(window);
        var dpi = GetDpiForWindow(hwnd);
        if (dpi == 0)
        {
            dpi = 96;
        }

        var scale = dpi / 96.0;
        var monitor = MonitorFromWindow(hwnd, 2);
        var info = new MonitorInfo { Size = Marshal.SizeOf<MonitorInfo>() };
        if (!GetMonitorInfo(monitor, ref info))
        {
            return;
        }

        var workWidth = info.Work.Right - info.Work.Left;
        var workHeight = info.Work.Bottom - info.Work.Top;
        var pxW = Math.Min((int)(width * scale), Math.Max(640, workWidth - 48));
        var pxH = Math.Min((int)(height * scale), Math.Max(480, workHeight - 48));
        var x = info.Work.Left + Math.Max(0, (workWidth - pxW) / 2);
        var y = info.Work.Top + Math.Max(0, (workHeight - pxH) / 2);
        // AppWindow.Move is overwritten by the shell's restored position.
        SetWindowPos(hwnd, IntPtr.Zero, x, y, pxW, pxH, 0x0004);
    }

    public static void PrepareTitleBar(Window window)
    {
        var titleBar = window.AppWindow.TitleBar;
        var transparent = Windows.UI.Color.FromArgb(0, 0, 0, 0);
        titleBar.ButtonBackgroundColor = transparent;
        titleBar.ButtonInactiveBackgroundColor = transparent;
    }

    /// <summary>
    /// Width, in device-independent pixels, to leave clear of the minimize,
    /// maximize, and close buttons. WinUI has reported that inset both in DIPs
    /// and in physical pixels; dividing a DIP value by the scale factor pulls
    /// the history and settings icons underneath the caption buttons.
    /// </summary>
    public static double CaptionInset(Window window)
    {
        var raw = window.AppWindow.TitleBar.RightInset;
        if (raw <= 0)
        {
            return 0;
        }

        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(window);
        var dpi = GetDpiForWindow(hwnd);
        var scale = dpi == 0 ? 1.0 : dpi / 96.0;
        var fromPixels = raw / scale;
        // Three Windows 11 caption buttons are about 46 DIP each.
        const double typical = 138;
        var dips = Math.Abs(raw - typical) <= Math.Abs(fromPixels - typical) ? raw : fromPixels;
        return dips + 12;
    }

    static void ApplyDpi(int dpi)
    {
        _minWidth = _logicalWidth * dpi / 96;
        _minHeight = _logicalHeight * dpi / 96;
    }

    static IntPtr Hook(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam)
    {
        if (message == WmDpiChanged)
        {
            var dpi = (int)((ulong)wParam.ToInt64() & 0xFFFF);
            if (dpi > 0)
            {
                ApplyDpi(dpi);
            }
        }
        else if (message == WmGetMinMaxInfo)
        {
            var info = Marshal.PtrToStructure<MinMaxInfo>(lParam);
            info.MinTrackSize.X = _minWidth;
            info.MinTrackSize.Y = _minHeight;
            Marshal.StructureToPtr(info, lParam, false);
        }

        return _previous == IntPtr.Zero ? IntPtr.Zero : CallWindowProc(_previous, hwnd, message, wParam, lParam);
    }

    delegate IntPtr WndProc(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    struct Point
    {
        public int X;
        public int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct MinMaxInfo
    {
        public Point Reserved;
        public Point MaxSize;
        public Point MaxPosition;
        public Point MinTrackSize;
        public Point MaxTrackSize;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct Rect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
    struct MonitorInfo
    {
        public int Size;
        public Rect Monitor;
        public Rect Work;
        public uint Flags;
    }

    [DllImport("user32.dll")]
    static extern uint GetDpiForWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);

    [DllImport("user32.dll", CharSet = CharSet.Auto)]
    static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo info);

    [DllImport("user32.dll", SetLastError = true)]
    static extern bool SetWindowPos(IntPtr hwnd, IntPtr insertAfter, int x, int y, int cx, int cy, uint flags);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    static extern IntPtr SetWindowLongPtr(IntPtr hwnd, int index, IntPtr newProc);

    [DllImport("user32.dll", EntryPoint = "CallWindowProcW")]
    static extern IntPtr CallWindowProc(IntPtr previous, IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);
}
