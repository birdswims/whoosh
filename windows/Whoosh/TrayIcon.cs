using System.Runtime.InteropServices;

namespace Whoosh;

/// Set when the user quits, or when Windows is ending the session, so closing
/// the window stops the engine instead of hiding it.
internal static class AppLifetime
{
    public static bool Quitting { get; set; }
}

/// Notification-area icon for an unpackaged WinUI app. Toasts need a package
/// identity this process may not have, so incoming requests use a balloon.
internal static class TrayIcon
{
    const uint NimAdd = 0x0;
    const uint NimModify = 0x1;
    const uint NimDelete = 0x2;
    const uint NifMessage = 0x1;
    const uint NifIcon = 0x2;
    const uint NifTip = 0x4;
    const uint NifInfo = 0x10;
    const uint NiifInfo = 0x1;
    const uint WmContextMenu = 0x007B;
    const uint WmLButtonUp = 0x0202;
    const uint WmLButtonDblClk = 0x0203;
    const uint WmRButtonUp = 0x0205;
    const uint WmUser = 0x0400;
    const uint NinBalloonUserClick = WmUser + 5;
    const uint CallbackMessage = 0x8000 + 20;
    const uint MfGrayed = 0x1;
    const uint MfDisabled = 0x2;
    const uint MfSeparator = 0x800;
    const uint TpmReturnCmd = 0x0100;
    const uint TpmRightButton = 0x0002;
    const uint TpmNoNotify = 0x0080;
    const int IdiApplication = 32512;
    const uint WsExToolWindow = 0x00000080;
    const uint WsPopup = 0x80000000;

    static readonly WndProc Proc = OnMessage;
    static IntPtr _hwnd;
    static IntPtr _icon;
    static bool _destroyIcon;
    static bool _added;

    public static void Add()
    {
        if (_added)
        {
            return;
        }

        if (_hwnd == IntPtr.Zero && !CreateMessageWindow())
        {
            return;
        }

        _icon = LoadAppIcon();
        var data = NewData();
        data.uFlags = NifMessage | NifIcon | NifTip;
        data.hIcon = _icon;
        data.szTip = "Whoosh";
        if (!Shell_NotifyIcon(NimAdd, ref data))
        {
            if (_destroyIcon && _icon != IntPtr.Zero)
            {
                DestroyIcon(_icon);
            }

            _icon = IntPtr.Zero;
            _destroyIcon = false;
            return;
        }

        _added = true;
    }

    public static void Remove()
    {
        if (_hwnd != IntPtr.Zero && _added)
        {
            var data = NewData();
            Shell_NotifyIcon(NimDelete, ref data);
        }

        _added = false;
        if (_destroyIcon && _icon != IntPtr.Zero)
        {
            DestroyIcon(_icon);
        }

        _icon = IntPtr.Zero;
        _destroyIcon = false;
    }

    public static void Notify(string title, string body)
    {
        if (!_added)
        {
            return;
        }

        var text = Clip(body, 255);
        if (text.Length == 0)
        {
            return;
        }

        var heading = Clip(title, 63);
        var data = NewData();
        data.uFlags = NifInfo;
        data.szInfoTitle = heading.Length == 0 ? "Whoosh" : heading;
        data.szInfo = text;
        data.dwInfoFlags = NiifInfo;
        Shell_NotifyIcon(NimModify, ref data);
    }

    static NotifyIconData NewData()
    {
        var data = new NotifyIconData
        {
            hwnd = _hwnd,
            uID = 1,
            uCallbackMessage = CallbackMessage,
            szTip = "",
            szInfo = "",
            szInfoTitle = "",
        };
        data.cbSize = Marshal.SizeOf<NotifyIconData>();
        return data;
    }

    static bool CreateMessageWindow()
    {
        var instance = GetModuleHandle(null);
        var wc = new WndClass
        {
            lpfnWndProc = Marshal.GetFunctionPointerForDelegate(Proc),
            hInstance = instance,
            lpszClassName = "Whoosh.Tray",
        };
        RegisterClass(ref wc);
        // A message-only window cannot take focus, so the tray menu would not open.
        // This popup is zero size and a tool window, so it stays off the taskbar.
        _hwnd = CreateWindowEx(
            WsExToolWindow,
            wc.lpszClassName,
            "Whoosh",
            WsPopup,
            0,
            0,
            0,
            0,
            IntPtr.Zero,
            IntPtr.Zero,
            instance,
            IntPtr.Zero);
        return _hwnd != IntPtr.Zero;
    }

    static IntPtr LoadAppIcon()
    {
        var exe = Environment.ProcessPath;
        if (!string.IsNullOrEmpty(exe))
        {
            var extracted = ExtractIcon(IntPtr.Zero, exe, 0);
            if (extracted != IntPtr.Zero && extracted != new IntPtr(1))
            {
                _destroyIcon = true;
                return extracted;
            }
        }

        _destroyIcon = false;
        return LoadIcon(IntPtr.Zero, new IntPtr(IdiApplication));
    }

    static void Open() => App.Main?.Reveal();

    static void Quit()
    {
        AppLifetime.Quitting = true;
        Remove();
        App.Main?.Close();
    }

    static bool _menuOpen;

    static void ShowMenu()
    {
        if (_hwnd == IntPtr.Zero || _menuOpen)
        {
            return;
        }

        _menuOpen = true;
        try
        {
            ShowMenuContents();
        }
        finally
        {
            _menuOpen = false;
        }
    }

    static void ShowMenuContents()
    {

        var peers = App.Model.Peers.Where(peer => peer.ShowClipboard).Take(24).ToArray();
        var menu = CreatePopupMenu();
        if (menu == IntPtr.Zero)
        {
            return;
        }

        AppendMenu(menu, 0, 1, "Open Whoosh");
        if (peers.Length == 0)
        {
            AppendMenu(menu, MfGrayed | MfDisabled, 0, "No trusted devices nearby");
        }
        else
        {
            for (var index = 0; index < peers.Length; index++)
            {
                var name = string.IsNullOrWhiteSpace(peers[index].Name) ? "device" : peers[index].Name.Replace("&", "&&");
                AppendMenu(menu, 0, (nuint)(10 + index), "Copy from " + name);
            }
        }

        AppendMenu(menu, MfSeparator, 0, null);
        AppendMenu(menu, 0, 2, "Quit Whoosh");
        _ = GetCursorPos(out var point);
        SetForegroundWindow(_hwnd);
        var command = TrackPopupMenu(menu, TpmReturnCmd | TpmNoNotify | TpmRightButton, point.X, point.Y, 0, _hwnd, IntPtr.Zero);
        PostMessage(_hwnd, 0, IntPtr.Zero, IntPtr.Zero);
        DestroyMenu(menu);
        if (command == 1)
        {
            Open();
        }
        else if (command == 2)
        {
            Quit();
        }
        else if (command >= 10 && command - 10 < peers.Length)
        {
            App.Model.CopyFromPeer(peers[command - 10]);
        }
    }

    static IntPtr OnMessage(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam)
    {
        if (message == CallbackMessage)
        {
            var code = unchecked((uint)lParam.ToInt64());
            if (code is WmLButtonUp or WmLButtonDblClk or NinBalloonUserClick)
            {
                Open();
            }
            else if (code is WmRButtonUp or WmContextMenu)
            {
                ShowMenu();
            }

            return IntPtr.Zero;
        }

        return DefWindowProc(hwnd, message, wParam, lParam);
    }

    static string Clip(string value, int max)
    {
        var text = value.Replace("\r", " ").Replace("\n", " ").Trim();
        return text.Length <= max ? text : text[..max];
    }

    delegate IntPtr WndProc(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct WndClass
    {
        public uint style;
        public IntPtr lpfnWndProc;
        public int cbClsExtra;
        public int cbWndExtra;
        public IntPtr hInstance;
        public IntPtr hIcon;
        public IntPtr hCursor;
        public IntPtr hbrBackground;
        public IntPtr lpszMenuName;
        public string lpszClassName;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct NotifyIconData
    {
        public int cbSize;
        public IntPtr hwnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
        public string szTip;
        public uint dwState;
        public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
        public string szInfo;
        public uint uTimeoutOrVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
        public string szInfoTitle;
        public uint dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct NativePoint
    {
        public int X;
        public int Y;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern bool Shell_NotifyIcon(uint message, ref NotifyIconData data);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr ExtractIcon(IntPtr instance, string file, uint index);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern ushort RegisterClass(ref WndClass wc);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateWindowEx(
        uint exStyle,
        string className,
        string windowName,
        uint style,
        int x,
        int y,
        int width,
        int height,
        IntPtr parent,
        IntPtr menu,
        IntPtr instance,
        IntPtr param);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr DefWindowProc(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr LoadIcon(IntPtr instance, IntPtr iconName);

    [DllImport("user32.dll", ExactSpelling = true)]
    static extern bool DestroyIcon(IntPtr icon);

    [DllImport("user32.dll", ExactSpelling = true)]
    static extern IntPtr CreatePopupMenu();

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern bool AppendMenu(IntPtr menu, uint flags, nuint id, string? text);

    [DllImport("user32.dll", ExactSpelling = true)]
    static extern int TrackPopupMenu(IntPtr menu, uint flags, int x, int y, int reserved, IntPtr hwnd, IntPtr rect);

    [DllImport("user32.dll", ExactSpelling = true)]
    static extern bool DestroyMenu(IntPtr menu);

    [DllImport("user32.dll", ExactSpelling = true)]
    static extern bool GetCursorPos(out NativePoint point);

    [DllImport("user32.dll", ExactSpelling = true)]
    static extern bool SetForegroundWindow(IntPtr hwnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr GetModuleHandle(string? module);
}
