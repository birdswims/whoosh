using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Storage.Pickers;
using Windows.System;

namespace Whoosh;

public sealed partial class SettingsWindow : UserControl
{
    readonly AppModel _model;
    bool _syncing;
    bool _nameDirty;

    public event EventHandler? DismissRequested;

    public SettingsWindow()
    {
        InitializeComponent();
        _model = App.Model;
        Root.DataContext = _model;
        _model.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName is nameof(AppModel.DeviceName) or nameof(AppModel.Native) or nameof(AppModel.Quickshare) or nameof(AppModel.Airdrop) or nameof(AppModel.SortMedia) or nameof(AppModel.RequirePin) or null)
            {
                Sync();
            }
        };
        Sync();
    }

    public void Shown()
    {
        Sync();
        _model.RefreshTrusted();
    }

    public void Commit()
    {
        _model.ApplyName(NameBox.Text);
        _nameDirty = false;
    }

    void Back_Click(object sender, RoutedEventArgs e) => DismissRequested?.Invoke(this, EventArgs.Empty);

    void Sync()
    {
        _syncing = true;
        if (!_nameDirty && NameBox.FocusState == FocusState.Unfocused)
        {
            NameBox.Text = _model.DeviceName;
        }

        NativeSwitch.IsOn = _model.Native;
        QuickSwitch.IsOn = _model.Quickshare;
        AirDropSwitch.IsOn = _model.Airdrop;
        SortSwitch.IsOn = _model.SortMedia;
        PinSwitch.IsOn = _model.RequirePin;
        _syncing = false;
    }

    void NameBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        _nameDirty = NameBox.Text != _model.DeviceName;
        if (e.Key == VirtualKey.Enter)
        {
            _model.ApplyName(NameBox.Text);
            _nameDirty = false;
        }
    }

    void NameBox_LostFocus(object sender, RoutedEventArgs e)
    {
        _model.ApplyName(NameBox.Text);
        _nameDirty = false;
    }

    async void ChooseFolder_Click(object sender, RoutedEventArgs e)
    {
        var picker = new FolderPicker();
        picker.FileTypeFilter.Add("*");
        picker.SuggestedStartLocation = PickerLocationId.DocumentsLibrary;
        var owner = App.Main;
        if (owner == null)
        {
            return;
        }

        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(owner));
        var folder = await picker.PickSingleFolderAsync();
        if (folder == null || string.IsNullOrEmpty(folder.Path))
        {
            return;
        }

        _model.SetSaveDir(folder.Path);
    }

    void Protocol_Toggled(object sender, RoutedEventArgs e)
    {
        if (_syncing)
        {
            return;
        }

        _model.SetProtocols(NativeSwitch.IsOn, QuickSwitch.IsOn, AirDropSwitch.IsOn, SortSwitch.IsOn, PinSwitch.IsOn);
    }

    void Copy_Click(object sender, RoutedEventArgs e) => _model.CopyFingerprint();

    void RemoveTrusted_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not Button button)
        {
            return;
        }

        var fingerprint = button.Tag as string;
        if (string.IsNullOrEmpty(fingerprint) && button.DataContext is TrustedDevice device)
        {
            fingerprint = device.Fingerprint;
        }

        if (!string.IsNullOrEmpty(fingerprint))
        {
            _model.RemoveTrusted(fingerprint);
        }
    }
}
