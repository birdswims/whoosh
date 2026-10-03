using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage;
using Windows.Storage.Pickers;

namespace Whoosh;

public sealed partial class MainWindow : Window
{
    readonly AppModel _model;
    bool _syncing;
    bool _sized;
    bool _refreshingPeers;

    public MainWindow()
    {
        InitializeComponent();
        _model = App.Model;
        Root.DataContext = _model;
        PeerList.ItemsSource = _model.Peers;
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(DragRegion);
        WindowChrome.PrepareTitleBar(this);
        WindowChrome.KeepAtLeast(this, 900, 640);
        _model.PropertyChanged += Model_Changed;
        Closed += (_, _) =>
        {
            App.SettingsWindow?.Close();
            App.ActivityWindow?.Close();
            _model.Shutdown();
        };
        Activated += (_, _) =>
        {
            if (_sized)
            {
                return;
            }

            _sized = true;
            WindowChrome.Place(this, 1040, 720);
            // Windows restores a saved position after activation. Re-center for a
            // short time so that restore does not leave the window off the work area.
            var timer = Microsoft.UI.Dispatching.DispatcherQueue.GetForCurrentThread().CreateTimer();
            timer.Interval = TimeSpan.FromMilliseconds(300);
            var ticks = 0;
            timer.IsRepeating = true;
            timer.Tick += (_, _) =>
            {
                WindowChrome.Place(this, 1040, 720);
                ApplyCaptionInset();
                ticks++;
                if (ticks >= 8)
                {
                    timer.Stop();
                }
            };
            timer.Start();
            ApplyCaptionInset();
            Root.SizeChanged += (_, _) => ApplyCaptionInset();
        };
    }

    void ApplyCaptionInset()
    {
        var inset = WindowChrome.CaptionInset(this);
        if (inset < 160)
        {
            inset = 192;
        }

        CaptionSpacer.Width = new GridLength(inset);
    }

    void Model_Changed(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(AppModel.PeerStamp))
        {
            PeerList.ItemsSource = _model.Peers;
            RestoreSelection();
        }

        if (e.PropertyName is nameof(AppModel.Receiving)
            or nameof(AppModel.CurrentOffer)
            or nameof(AppModel.TrustPeer)
            or nameof(AppModel.EngineDown)
            or null)
        {
            SyncChrome();
        }
    }

    void SyncChrome()
    {
        _syncing = true;
        ReceivingToggle.IsOn = _model.Receiving;
        _syncing = false;
        StatusDot.Fill = _model.Receiving ? SignalBrush() : SecondaryBrush();
        SyncOffer();
        SyncTrust();
    }

    void SyncOffer()
    {
        var offer = _model.CurrentOffer;
        if (offer == null)
        {
            return;
        }

        OfferPeer.Text = offer.Peer;
        var count = offer.Files.Count;
        var noun = count == 1 ? "file" : "files";
        var total = offer.TotalBytes;
        OfferSummary.Text = offer.SizeUnknown
            ? $"{Format.Protocol(offer.Via)} · {count} {noun}"
            : $"{Format.Protocol(offer.Via)} · {count} {noun} · {Format.Size(total)}";
        OfferFiles.ItemsSource = offer.Files;
        var quickPin = offer.Via == "quickshare" && !string.IsNullOrEmpty(offer.Pin);
        OfferPinPanel.Visibility = quickPin ? Visibility.Visible : Visibility.Collapsed;
        OfferPin.Text = offer.Pin ?? "";
        OfferProtected.Visibility = offer.Via == "whoosh" && offer.Pin != null
            ? Visibility.Visible
            : Visibility.Collapsed;
    }

    void SyncTrust()
    {
        var peer = _model.TrustPeer;
        if (peer == null)
        {
            return;
        }

        TrustTitle.Text = $"Trust {peer.Name}?";
        if (!string.IsNullOrEmpty(peer.Fingerprint))
        {
            TrustBody.Text = $"Compare this fingerprint with the one shown on {peer.Name}. Later sends to this address check it again.";
            TrustPrint.Text = Format.Fingerprint(peer.Fingerprint);
            TrustPrint.Visibility = Visibility.Visible;
        }
        else
        {
            TrustBody.Text = $"This device did not share a fingerprint. Trusting it remembers {peer.Address} for later sends.";
            TrustPrint.Visibility = Visibility.Collapsed;
        }
    }

    void RestoreSelection()
    {
        _refreshingPeers = true;
        Peer? match = null;
        if (_model.SelectedPeerId != null)
        {
            foreach (var peer in _model.Peers)
            {
                if (peer.Id == _model.SelectedPeerId)
                {
                    match = peer;
                    break;
                }
            }
        }

        PeerList.SelectedItem = match;
        _refreshingPeers = false;
    }

    void Receiving_Toggled(object sender, RoutedEventArgs e)
    {
        if (_syncing || ReceivingToggle.IsOn == _model.Receiving)
        {
            return;
        }

        _model.SetReceiving(ReceivingToggle.IsOn);
    }

    async void ChooseFiles_Click(object sender, RoutedEventArgs e) => await ChooseFilesAsync();

    void ChooseFiles_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        _ = ChooseFilesAsync();
    }

    async Task ChooseFilesAsync()
    {
        var picker = new FileOpenPicker();
        picker.FileTypeFilter.Add("*");
        picker.SuggestedStartLocation = PickerLocationId.Downloads;
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        var files = await picker.PickMultipleFilesAsync();
        if (files == null || files.Count == 0)
        {
            return;
        }

        _model.AddPaths(files.Select(file => file.Path));
    }

    void Send_Click(object sender, RoutedEventArgs e) => _model.Send();

    void Send_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        if (_model.CanSend)
        {
            _model.Send();
        }
    }

    void ToggleReceive_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        _model.SetReceiving(!_model.Receiving);
    }

    void Activity_Click(object sender, RoutedEventArgs e)
    {
        if (App.ActivityWindow == null)
        {
            var window = new ActivityWindow();
            window.Closed += (_, _) =>
            {
                if (ReferenceEquals(App.ActivityWindow, window))
                {
                    App.ActivityWindow = null;
                }
            };
            App.ActivityWindow = window;
        }

        App.ActivityWindow.Activate();
    }

    void Settings_Click(object sender, RoutedEventArgs e)
    {
        if (App.SettingsWindow == null)
        {
            var window = new SettingsWindow();
            window.Closed += (_, _) =>
            {
                if (ReferenceEquals(App.SettingsWindow, window))
                {
                    App.SettingsWindow = null;
                }
            };
            App.SettingsWindow = window;
        }

        App.SettingsWindow.Activate();
    }

    void SaveFolder_Click(object sender, RoutedEventArgs e) => _model.RevealSaveFolder();

    void RemoveFile_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: string path })
        {
            _model.RemoveFile(path);
        }
    }

    void PeerList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_refreshingPeers)
        {
            return;
        }

        if (PeerList.SelectedItem is Peer peer)
        {
            _model.Select(peer);
        }
    }

    void Root_DragOver(object sender, DragEventArgs e)
    {
        e.AcceptedOperation = DataPackageOperation.Copy;
        e.DragUIOverride.Caption = "Add files";
        e.DragUIOverride.IsCaptionVisible = true;
        e.DragUIOverride.IsGlyphVisible = false;
        DropWell.BorderBrush = SignalBrush();
        DropLabel.Text = "Drop to add";
    }

    void Root_DragLeave(object sender, DragEventArgs e)
    {
        DropWell.BorderBrush = SecondaryBrush();
        DropLabel.Text = "Drop files";
    }

    async void Root_Drop(object sender, DragEventArgs e)
    {
        Root_DragLeave(sender, e);
        if (!e.DataView.Contains(StandardDataFormats.StorageItems))
        {
            return;
        }

        var items = await e.DataView.GetStorageItemsAsync();
        _model.AddPaths(items.Select(item => item.Path).Where(path => !string.IsNullOrEmpty(path))!);
    }

    void Root_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != Windows.System.VirtualKey.Escape)
        {
            return;
        }

        if (_model.CurrentOffer != null)
        {
            _model.DeclineCurrentOffer();
            e.Handled = true;
        }
        else if (_model.TrustPeer != null)
        {
            _model.CancelTrust();
            e.Handled = true;
        }
    }

    void OfferSmoke_Tapped(object sender, TappedRoutedEventArgs e)
    {
        if (ReferenceEquals(e.OriginalSource, OfferLayer))
        {
            _model.DeclineCurrentOffer();
        }
    }

    void TrustSmoke_Tapped(object sender, TappedRoutedEventArgs e)
    {
        if (ReferenceEquals(e.OriginalSource, TrustLayer))
        {
            _model.CancelTrust();
        }
    }

    void Card_Tapped(object sender, TappedRoutedEventArgs e) => e.Handled = true;

    void Accept_Click(object sender, RoutedEventArgs e) => _model.AcceptCurrentOffer();

    void Decline_Click(object sender, RoutedEventArgs e) => _model.DeclineCurrentOffer();

    void TrustSend_Click(object sender, RoutedEventArgs e) => _model.ConfirmTrust();

    void TrustCancel_Click(object sender, RoutedEventArgs e) => _model.CancelTrust();

    void Retry_Click(object sender, RoutedEventArgs e) => _model.Retry();

    void Banner_Closed(InfoBar sender, InfoBarClosedEventArgs args) => _model.ClearBanner();

    SolidColorBrush SignalBrush()
    {
        var dark = Root.ActualTheme == ElementTheme.Dark;
        var color = dark
            ? Windows.UI.Color.FromArgb(255, 148, 214, 189)
            : Windows.UI.Color.FromArgb(255, 33, 115, 94);
        return new SolidColorBrush(color);
    }

    static SolidColorBrush SecondaryBrush()
        => new(Windows.UI.Color.FromArgb(80, 128, 128, 128));
}
