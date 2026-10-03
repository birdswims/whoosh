using System.Collections.ObjectModel;
using System.Diagnostics;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.ApplicationModel.DataTransfer;

namespace Whoosh;

public sealed class SendFile : Observable
{
    public SendFile(string path)
    {
        Path = path;
        Name = System.IO.Path.GetFileName(path);
        Glyph = Format.Glyph(Name, null);
    }

    public string Path { get; }
    public string Name { get; }
    public string Glyph { get; }

    ImageSource? _thumbnail;
    public ImageSource? Thumbnail
    {
        get => _thumbnail;
        set
        {
            if (ReferenceEquals(_thumbnail, value))
            {
                return;
            }

            _thumbnail = value;
            Raise(nameof(Thumbnail));
            Raise(nameof(HasThumbnail));
            Raise(nameof(ShowGlyph));
        }
    }

    public bool HasThumbnail => _thumbnail != null;
    public bool ShowGlyph => _thumbnail == null;
}

public sealed class ActivityItem : Observable
{
    public string Id { get; set; } = "";
    public string Direction { get; set; } = "";
    public string Peer { get; set; } = "";
    public string Via { get; set; } = "";
    public List<string> Paths { get; } = [];

    string _state = "";
    public string State
    {
        get => _state;
        set
        {
            Set(ref _state, value);
            Raise(nameof(Subtitle));
            Raise(nameof(CanShow));
            Raise(nameof(IsWorking));
            Raise(nameof(IsDone));
            Raise(nameof(IsFailed));
            Raise(nameof(IsOther));
        }
    }

    string _title = "";
    public string Title
    {
        get => _title;
        set => Set(ref _title, value);
    }

    string _detail = "";
    public string Detail
    {
        get => _detail;
        set
        {
            Set(ref _detail, value);
            Raise(nameof(Subtitle));
        }
    }

    ulong? _bytes;
    public ulong? Bytes
    {
        get => _bytes;
        set
        {
            Set(ref _bytes, value);
            Raise(nameof(Subtitle));
        }
    }

    public string Subtitle => Bytes is ulong bytes && State == "done" ? $"{Detail} · {Format.Size(bytes)}" : Detail;
    public bool CanShow => State == "done" && Paths.Count > 0;
    public bool IsWorking => State == "working";
    public bool IsDone => State == "done";
    public bool IsFailed => State == "failed";
    public bool IsOther => !IsWorking && !IsDone && !IsFailed;

    public static ActivityItem From(WireEvent ev)
    {
        var item = new ActivityItem
        {
            Id = ev.Id ?? Guid.NewGuid().ToString("n"),
            Direction = ev.Direction ?? "",
            State = ev.State ?? "",
            Title = ev.Title ?? "",
            Detail = ev.Detail ?? "",
            Peer = ev.Peer ?? "",
            Via = ev.Via ?? "",
            Bytes = ev.Bytes,
        };
        if (ev.Paths != null)
        {
            item.Paths.AddRange(ev.Paths);
        }

        return item;
    }

    public void Apply(WireEvent ev)
    {
        if (!string.IsNullOrEmpty(ev.Direction))
        {
            Direction = ev.Direction;
        }

        if (!string.IsNullOrEmpty(ev.State))
        {
            State = ev.State;
        }

        if (!string.IsNullOrEmpty(ev.Title))
        {
            Title = ev.Title;
        }

        if (ev.Detail != null)
        {
            Detail = ev.Detail;
        }

        if (!string.IsNullOrEmpty(ev.Peer))
        {
            Peer = ev.Peer;
        }

        if (!string.IsNullOrEmpty(ev.Via))
        {
            Via = ev.Via;
        }

        if (ev.Bytes != null)
        {
            Bytes = ev.Bytes;
        }

        if (ev.Paths is { Count: > 0 })
        {
            Paths.Clear();
            Paths.AddRange(ev.Paths);
            Raise(nameof(CanShow));
        }
    }
}

public sealed class SavedSettings
{
    public string? Name { get; set; }
    public string? Dir { get; set; }
    public bool? Native { get; set; }
    public bool? Quickshare { get; set; }
    public bool? Airdrop { get; set; }
    public bool? SortMedia { get; set; }
    public bool? RequirePin { get; set; }
    public bool? Receiving { get; set; }
    public string? SelectedPeer { get; set; }
}

public sealed class AppModel : Observable
{
    readonly Engine _engine = new();
    readonly List<Offer> _offerQueue = [];
    readonly HashSet<string> _responded = [];
    readonly SavedSettings _settings = ReadSettings();
    DispatcherQueue? _queue;
    DispatcherQueueTimer? _bannerTimer;
    bool _wired;
    bool _didShutdown;
    bool _appliedPrefs;
    int _nextId;
    string? _selectedPeerId;

    public ObservableCollection<Peer> Peers { get; } = [];
    public ObservableCollection<SendFile> Files { get; } = [];
    public ObservableCollection<ActivityItem> Activity { get; } = [];

    public string Version { get; private set; } = "";
    public bool Receiving { get; private set; }
    public string DeviceName { get; private set; } = "";
    public string SaveDir { get; private set; } = "";
    public string Fingerprint { get; private set; } = "";
    public string? Pin { get; private set; }
    public bool Native { get; private set; } = true;
    public bool Quickshare { get; private set; } = true;
    public bool Airdrop { get; private set; } = true;
    public bool SortMedia { get; private set; }
    public bool RequirePin { get; private set; }
    public string Visibility { get; private set; } = "";
    public string? Address { get; private set; }
    public List<string> Warnings { get; private set; } = [];
    public bool SawPeers { get; private set; }
    public int PeerStamp { get; private set; }
    public string OutgoingPin { get; set; } = "";
    public Offer? CurrentOffer { get; private set; }
    public Peer? TrustPeer { get; private set; }
    public string Banner { get; private set; } = "";
    public bool EngineDown { get; private set; }
    public string EngineError { get; private set; } = "";
    public bool Sending { get; private set; }
    bool _ready;

    public string? SelectedPeerId => _selectedPeerId;
    public Peer? SelectedPeer => Peers.FirstOrDefault(peer => peer.Id == _selectedPeerId);
    public bool CanSend => SelectedPeer != null && Files.Count > 0 && !EngineDown && CurrentOffer == null && TrustPeer == null && !Sending;
    public bool HasFiles => Files.Count > 0;
    public bool HasOffer => CurrentOffer != null;
    public bool HasTrust => TrustPeer != null;
    public bool HasBanner => Banner.Length > 0;
    public bool HasWarnings => Warnings.Count > 0;
    public bool HasActivity => Activity.Count > 0;
    public bool CanClearActivity => Activity.Any(item => item.State != "working");
    public bool ShowOutgoingPin => SelectedPeer?.Via == "whoosh";
    public bool ShowPin => RequirePin && !string.IsNullOrEmpty(Pin);
    public bool ShowPeerList => Peers.Count > 0;
    public bool ShowNearbyMessage => Peers.Count == 0;
    public bool ShowLooking => !SawPeers;
    public string NearbyMessage => SawPeers ? "No devices nearby" : "Looking…";
    public string WarningsText => string.Join(' ', Warnings);
    public string SaveLabel => string.IsNullOrEmpty(SaveDir) ? "Choose a folder" : Format.Abbreviate(SaveDir);
    public string GroupedFingerprint => Fingerprint.Length == 0 ? "Waiting for the engine." : Format.Fingerprint(Fingerprint);
    public string ReceivingLabel => Receiving ? "Receiving" : "Paused";

    public string StatusLine
    {
        get
        {
            if (EngineDown)
            {
                return "Stopped";
            }

            if (!_ready)
            {
                return "Starting…";
            }

            return Receiving ? "On this network" : "Not receiving";
        }
    }

    public string SendTitle
    {
        get
        {
            if (Sending)
            {
                return "Sending…";
            }

            var name = SelectedPeer?.Name;
            return string.IsNullOrEmpty(name) ? "Send" : $"Send to {name}";
        }
    }

    public string Hint
    {
        get
        {
            if (Files.Count == 0)
            {
                return "Drop files or folders here, or choose them.";
            }

            var peer = SelectedPeer;
            if (peer == null)
            {
                return "Choose a device nearby.";
            }

            if (Sending)
            {
                var active = Activity.FirstOrDefault(item => item.Direction == "out" && item.State == "working");
                return string.IsNullOrEmpty(active?.Detail) ? "Preparing transfer…" : active.Detail;
            }

            if (peer.Via == "whoosh" && !peer.Trusted)
            {
                return "You confirm this device’s fingerprint before the first send.";
            }

            var noun = Files.Count == 1 ? "file" : "files";
            return $"{Files.Count} {noun} to {peer.Name}.";
        }
    }

    public AppModel()
    {
        _selectedPeerId = _settings.SelectedPeer;
    }

    public void Start()
    {
        _queue = DispatcherQueue.GetForCurrentThread();
        if (!_wired)
        {
            _wired = true;
            _engine.EventReceived += ev => _queue?.TryEnqueue(() => Handle(ev));
            _engine.Exited += message => _queue?.TryEnqueue(() => HandleExit(message));
        }

        _didShutdown = false;
        EngineDown = false;
        _appliedPrefs = false;
        Raise(nameof(EngineDown));
        _engine.Start();
    }

    public void Retry()
    {
        EngineDown = false;
        EngineError = "";
        _appliedPrefs = false;
        Raise(nameof(EngineDown));
        Raise(nameof(EngineError));
        Touch();
        _engine.Start();
    }

    public void Shutdown()
    {
        if (_didShutdown)
        {
            return;
        }

        _didShutdown = true;
        _engine.StopAndWait();
    }

    public void SetReceiving(bool enabled)
    {
        Receiving = enabled;
        Raise(nameof(Receiving));
        _settings.Receiving = enabled;
        WriteSettings();
        Touch();
        _engine.Send(new Command { Id = NextId(), Op = "receive", Enabled = enabled });
    }

    public void ApplyName(string raw)
    {
        var trimmed = raw.Trim();
        if (trimmed.Length == 0 || trimmed == DeviceName)
        {
            return;
        }

        DeviceName = trimmed.Length > 64 ? trimmed[..64] : trimmed;
        Raise(nameof(DeviceName));
        _settings.Name = DeviceName;
        WriteSettings();
        PushConfig();
    }

    public void SetSaveDir(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return;
        }

        SaveDir = path;
        Raise(nameof(SaveDir));
        _settings.Dir = path;
        WriteSettings();
        Touch();
        PushConfig();
    }

    public void SetProtocols(bool native, bool quickshare, bool airdrop, bool sortMedia, bool requirePin)
    {
        Native = native;
        Quickshare = quickshare;
        Airdrop = airdrop;
        SortMedia = sortMedia;
        RequirePin = requirePin;
        Raise(nameof(Native));
        Raise(nameof(Quickshare));
        Raise(nameof(Airdrop));
        Raise(nameof(SortMedia));
        Raise(nameof(RequirePin));
        _settings.Native = native;
        _settings.Quickshare = quickshare;
        _settings.Airdrop = airdrop;
        _settings.SortMedia = sortMedia;
        _settings.RequirePin = requirePin;
        WriteSettings();
        Touch();
        PushConfig();
    }

    public void RevealSaveFolder()
    {
        if (string.IsNullOrEmpty(SaveDir))
        {
            return;
        }

        Directory.CreateDirectory(SaveDir);
        Process.Start(new ProcessStartInfo { FileName = SaveDir, UseShellExecute = true });
    }

    public void AddPaths(IEnumerable<string> paths)
    {
        var seen = new HashSet<string>(Files.Select(file => file.Path), StringComparer.OrdinalIgnoreCase);
        var added = new List<string>();
        var truncated = false;
        foreach (var path in paths)
        {
            foreach (var file in Expand(path))
            {
                if (Files.Count >= 500)
                {
                    truncated = true;
                    break;
                }

                if (!seen.Add(file))
                {
                    continue;
                }

                Files.Add(new SendFile(file));
                added.Add(file);
            }
        }

        LoadThumbnails(added);
        Touch();
        if (truncated)
        {
            ShowBanner("Whoosh sends up to 500 files at a time.");
        }
    }

    public void RemoveFile(string path)
    {
        for (var index = Files.Count - 1; index >= 0; index--)
        {
            if (string.Equals(Files[index].Path, path, StringComparison.OrdinalIgnoreCase))
            {
                Files.RemoveAt(index);
            }
        }

        Touch();
    }

    public void Select(Peer peer)
    {
        if (_selectedPeerId == peer.Id)
        {
            Touch();
            return;
        }

        _selectedPeerId = peer.Id;
        _settings.SelectedPeer = peer.Id;
        WriteSettings();
        Raise(nameof(SelectedPeerId));
        Raise(nameof(SelectedPeer));
        Touch();
    }

    public void Send()
    {
        if (Sending || SelectedPeer is not Peer peer || Files.Count == 0)
        {
            return;
        }

        if (peer.Via == "whoosh" && !peer.Trusted)
        {
            TrustPeer = peer;
            Raise(nameof(TrustPeer));
            Touch();
            return;
        }

        BeginSend(peer, peer.Via == "whoosh");
    }

    public void ConfirmTrust()
    {
        if (TrustPeer is not Peer peer)
        {
            return;
        }

        TrustPeer = null;
        Raise(nameof(TrustPeer));
        BeginSend(peer, trust: true);
    }

    public void CancelTrust()
    {
        TrustPeer = null;
        Raise(nameof(TrustPeer));
        Touch();
    }

    public void AcceptCurrentOffer() => Respond(true);

    public void DeclineCurrentOffer() => Respond(false);

    public void ShowActivity(ActivityItem item)
    {
        if (item.Paths.Count == 0)
        {
            return;
        }

        var path = item.Paths[0];
        Process.Start(new ProcessStartInfo
        {
            FileName = "explorer.exe",
            Arguments = "/select,\"" + path + "\"",
            UseShellExecute = true,
        });
    }

    public void ClearActivity()
    {
        for (var index = Activity.Count - 1; index >= 0; index--)
        {
            if (Activity[index].State != "working")
            {
                Activity.RemoveAt(index);
            }
        }

        Touch();
    }

    public void CopyFingerprint()
    {
        if (Fingerprint.Length == 0)
        {
            return;
        }

        var package = new DataPackage();
        package.SetText(Fingerprint);
        Clipboard.SetContent(package);
        ShowBanner("Fingerprint copied.");
    }

    public void ClearBanner()
    {
        Banner = "";
        Raise(nameof(Banner));
        Touch();
    }

    void BeginSend(Peer peer, bool trust)
    {
        Sending = true;
        Raise(nameof(Sending));
        Touch();
        var pin = OutgoingPin.Trim();
        _engine.Send(new Command
        {
            Id = NextId(),
            Op = "send",
            Via = peer.Via,
            Target = peer.Address,
            Files = Files.Select(file => file.Path).ToList(),
            Pin = peer.Via == "whoosh" && pin.Length > 0 ? pin : null,
            Trust = peer.Via == "whoosh" ? trust : null,
            Fingerprint = peer.Via == "whoosh" ? peer.Fingerprint : null,
            PeerName = peer.Name,
        });
    }

    void Respond(bool accept)
    {
        if (CurrentOffer is not Offer offer)
        {
            return;
        }

        if (_responded.Add(offer.Id))
        {
            _engine.Send(new Command { Id = NextId(), Op = "decide", Accept = accept, Offer = offer.Id });
        }

        FinishOffer(offer.Id);
    }

    void FinishOffer(string id)
    {
        _offerQueue.RemoveAll(offer => offer.Id == id);
        if (CurrentOffer?.Id != id)
        {
            return;
        }

        CurrentOffer = _offerQueue.Count == 0 ? null : _offerQueue[0];
        if (CurrentOffer != null)
        {
            _offerQueue.RemoveAt(0);
        }

        Raise(nameof(CurrentOffer));
        Touch();
    }

    void PushConfig()
    {
        _engine.Send(new Command
        {
            Id = NextId(),
            Op = "configure",
            Name = string.IsNullOrEmpty(DeviceName) ? null : DeviceName,
            Dir = string.IsNullOrEmpty(SaveDir) ? null : SaveDir,
            Native = Native,
            Quickshare = Quickshare,
            Airdrop = Airdrop,
            SortMedia = SortMedia,
            RequirePin = RequirePin,
        });
    }

    void Handle(WireEvent ev)
    {
        switch (ev.Ev)
        {
            case "hello":
                if (!string.IsNullOrEmpty(ev.Version))
                {
                    Version = ev.Version;
                    Raise(nameof(Version));
                }

                if (!string.IsNullOrEmpty(ev.Device) && DeviceName.Length == 0)
                {
                    DeviceName = ev.Device;
                    Raise(nameof(DeviceName));
                }

                if (!string.IsNullOrEmpty(ev.Fingerprint))
                {
                    Fingerprint = ev.Fingerprint;
                    Raise(nameof(Fingerprint));
                }

                _ready = true;
                ApplySavedPreferences();
                Touch();
                break;
            case "status":
                ApplyStatus(ev);
                _ready = true;
                ApplySavedPreferences();
                Touch();
                break;
            case "ack":
                if (ev.Ok == false && !string.IsNullOrEmpty(ev.Error))
                {
                    Sending = false;
                    Raise(nameof(Sending));
                    ShowBanner(ev.Error);
                    Touch();
                }

                break;
            case "peers":
                SetPeers(ev.Peers ?? []);
                break;
            case "offer":
                if (string.IsNullOrEmpty(ev.Id))
                {
                    break;
                }

                var offer = new Offer
                {
                    Id = ev.Id,
                    Via = ev.Via ?? "",
                    Peer = string.IsNullOrEmpty(ev.Peer) ? "Someone" : ev.Peer,
                    Pin = ev.Pin,
                    Files = ev.Files ?? [],
                };
                if (CurrentOffer == null)
                {
                    CurrentOffer = offer;
                }
                else
                {
                    _offerQueue.Add(offer);
                }

                Raise(nameof(CurrentOffer));
                Touch();
                break;
            case "offer_resolved":
                if (!string.IsNullOrEmpty(ev.Id))
                {
                    _responded.Add(ev.Id);
                    FinishOffer(ev.Id);
                }

                break;
            case "activity":
                Record(ev);
                break;
        }
    }

    void ApplyStatus(WireEvent ev)
    {
        if (ev.Receiving is bool receiving)
        {
            Receiving = receiving;
            Raise(nameof(Receiving));
        }

        if (!string.IsNullOrEmpty(ev.Name))
        {
            DeviceName = ev.Name;
            Raise(nameof(DeviceName));
        }

        if (!string.IsNullOrEmpty(ev.Dir))
        {
            SaveDir = ev.Dir;
            Raise(nameof(SaveDir));
        }

        if (!string.IsNullOrEmpty(ev.Fingerprint))
        {
            Fingerprint = ev.Fingerprint;
            Raise(nameof(Fingerprint));
        }

        Pin = ev.Pin;
        Raise(nameof(Pin));
        if (ev.Native is bool native)
        {
            Native = native;
            Raise(nameof(Native));
        }

        if (ev.Quickshare is bool quickshare)
        {
            Quickshare = quickshare;
            Raise(nameof(Quickshare));
        }

        if (ev.Airdrop is bool airdrop)
        {
            Airdrop = airdrop;
            Raise(nameof(Airdrop));
        }

        if (ev.SortMedia is bool sortMedia)
        {
            SortMedia = sortMedia;
            Raise(nameof(SortMedia));
        }

        if (ev.RequirePin is bool requirePin)
        {
            RequirePin = requirePin;
            Raise(nameof(RequirePin));
        }

        if (ev.Visibility != null)
        {
            Visibility = ev.Visibility;
            Raise(nameof(Visibility));
        }

        Address = ev.Address;
        Raise(nameof(Address));
        Warnings = ev.Warnings ?? [];
        Raise(nameof(Warnings));
    }

    void ApplySavedPreferences()
    {
        if (_appliedPrefs)
        {
            return;
        }

        _appliedPrefs = true;
        var saved = _settings;
        var command = new Command { Id = NextId(), Op = "configure" };
        var any = false;
        if (!string.IsNullOrEmpty(saved.Name))
        {
            command.Name = saved.Name;
            any = true;
        }

        if (!string.IsNullOrEmpty(saved.Dir))
        {
            command.Dir = saved.Dir;
            any = true;
        }

        if (saved.Native is bool native)
        {
            command.Native = native;
            any = true;
        }

        if (saved.Quickshare is bool quickshare)
        {
            command.Quickshare = quickshare;
            any = true;
        }

        if (saved.Airdrop is bool airdrop)
        {
            command.Airdrop = airdrop;
            any = true;
        }

        if (saved.SortMedia is bool sortMedia)
        {
            command.SortMedia = sortMedia;
            any = true;
        }

        if (saved.RequirePin is bool requirePin)
        {
            command.RequirePin = requirePin;
            any = true;
        }

        if (any)
        {
            _engine.Send(command);
        }

        var enabled = saved.Receiving ?? true;
        _engine.Send(new Command { Id = NextId(), Op = "receive", Enabled = enabled });
    }

    void SetPeers(IReadOnlyList<Peer> incoming)
    {
        var selected = _selectedPeerId;
        if (selected != null && !incoming.Any(peer => peer.Id == selected))
        {
            var previous = Peers.FirstOrDefault(peer => peer.Id == selected);
            var match = incoming.FirstOrDefault(peer =>
            {
                if ($"{peer.Via}|{peer.Address}" == selected)
                {
                    return true;
                }

                if (previous == null || previous.Via != peer.Via)
                {
                    return false;
                }

                if (previous.Name == peer.Name)
                {
                    return true;
                }

                return previous.Via == "airdrop" && incoming.Count(item => item.Via == "airdrop") == 1;
            });
            if (match != null)
            {
                _selectedPeerId = match.Id;
                _settings.SelectedPeer = match.Id;
                WriteSettings();
            }
        }

        Peers.Clear();
        foreach (var peer in incoming)
        {
            Peers.Add(peer);
        }

        SawPeers = true;
        if (_selectedPeerId == null && Peers.Count > 0)
        {
            _selectedPeerId = Peers[0].Id;
        }

        Raise(nameof(SawPeers));
        Raise(nameof(SelectedPeer));
        PeerStamp++;
        Raise(nameof(PeerStamp));
        Touch();
    }

    void Record(WireEvent ev)
    {
        var incoming = ActivityItem.From(ev);
        var index = -1;
        for (var i = 0; i < Activity.Count; i++)
        {
            if (Activity[i].Id == incoming.Id)
            {
                index = i;
                break;
            }
        }

        if (index >= 0)
        {
            Activity[index].Apply(ev);
        }
        else if (ev.Direction == "in" && (ev.State == "done" || ev.State == "failed"))
        {
            var peer = ev.Peer ?? "";
            var working = -1;
            for (var i = 0; i < Activity.Count; i++)
            {
                var row = Activity[i];
                if (row.Direction == "in" && row.State == "working" && (row.Peer == peer || row.Peer.Length == 0 || peer.Length == 0))
                {
                    working = i;
                    break;
                }
            }

            if (working >= 0)
            {
                Activity[working].Apply(ev);
            }
            else
            {
                Activity.Insert(0, incoming);
            }
        }
        else
        {
            Activity.Insert(0, incoming);
        }

        while (Activity.Count > 40)
        {
            Activity.RemoveAt(Activity.Count - 1);
        }

        if (ev.Direction == "out" && (ev.State == "done" || ev.State == "failed"))
        {
            Sending = false;
            Raise(nameof(Sending));
        }

        if (ev.Direction == "out" && ev.State == "done")
        {
            var name = ev.Peer ?? "";
            var via = ev.Via ?? "";
            foreach (var peer in Peers)
            {
                if (peer.Name == name && peer.Via == via)
                {
                    peer.Trusted = true;
                }
            }

            if (Banner.Length > 0)
            {
                ClearBanner();
            }
        }

        if (ev.State == "failed")
        {
            var message = !string.IsNullOrEmpty(ev.Detail) ? ev.Detail : (ev.Title ?? "Transfer failed");
            ShowBanner(message);
        }

        Touch();
    }

    void HandleExit(string message)
    {
        if (_didShutdown)
        {
            return;
        }

        EngineDown = true;
        Sending = false;
        Receiving = false;
        Raise(nameof(EngineDown));
        Raise(nameof(Sending));
        Raise(nameof(Receiving));
        var line = message.Split('\r', '\n').LastOrDefault(part => !string.IsNullOrWhiteSpace(part))?.Trim() ?? "";
        EngineError = line.Length == 0 ? "Whoosh stopped unexpectedly." : line.Length > 280 ? line[..280] : line;
        Raise(nameof(EngineError));
        Touch();
    }

    void ShowBanner(string text)
    {
        Banner = text;
        Raise(nameof(Banner));
        Touch();
        _bannerTimer?.Stop();
        if (_queue == null)
        {
            return;
        }

        _bannerTimer = _queue.CreateTimer();
        _bannerTimer.Interval = TimeSpan.FromSeconds(6);
        _bannerTimer.IsRepeating = false;
        _bannerTimer.Tick += (_, _) => ClearBanner();
        _bannerTimer.Start();
    }

    void LoadThumbnails(IEnumerable<string> paths)
    {
        foreach (var path in paths)
        {
            if (!Format.IsImage(path))
            {
                continue;
            }

            var file = Files.FirstOrDefault(item => string.Equals(item.Path, path, StringComparison.OrdinalIgnoreCase));
            if (file == null || file.Thumbnail != null)
            {
                continue;
            }

            try
            {
                var image = new BitmapImage { DecodePixelWidth = 160 };
                image.ImageFailed += (_, _) => _queue?.TryEnqueue(() => file.Thumbnail = null);
                image.UriSource = new Uri(path);
                file.Thumbnail = image;
            }
            catch
            {
                // The glyph stays.
            }
        }
    }

    string NextId()
    {
        _nextId++;
        return _nextId.ToString();
    }

    void Touch()
    {
        Raise(nameof(CanSend));
        Raise(nameof(SendTitle));
        Raise(nameof(Hint));
        Raise(nameof(ShowOutgoingPin));
        Raise(nameof(StatusLine));
        Raise(nameof(ReceivingLabel));
        Raise(nameof(ShowPin));
        Raise(nameof(SaveLabel));
        Raise(nameof(GroupedFingerprint));
        Raise(nameof(HasWarnings));
        Raise(nameof(WarningsText));
        Raise(nameof(ShowPeerList));
        Raise(nameof(ShowNearbyMessage));
        Raise(nameof(NearbyMessage));
        Raise(nameof(ShowLooking));
        Raise(nameof(HasFiles));
        Raise(nameof(CanClearActivity));
        Raise(nameof(HasBanner));
        Raise(nameof(HasOffer));
        Raise(nameof(HasTrust));
        Raise(nameof(HasActivity));
    }

    void WriteSettings()
    {
        try
        {
            var path = SettingsPath();
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path, System.Text.Json.JsonSerializer.Serialize(_settings));
        }
        catch
        {
            // Preferences are a convenience. A transfer still runs without them.
        }
    }

    static SavedSettings ReadSettings()
    {
        try
        {
            var path = SettingsPath();
            if (!File.Exists(path))
            {
                return new SavedSettings();
            }

            return System.Text.Json.JsonSerializer.Deserialize<SavedSettings>(File.ReadAllText(path)) ?? new SavedSettings();
        }
        catch
        {
            return new SavedSettings();
        }
    }

    static string SettingsPath()
    {
        return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "Whoosh",
            "settings.json");
    }

    static List<string> Expand(string path)
    {
        var found = new List<string>();
        if (!File.Exists(path) && !Directory.Exists(path))
        {
            return found;
        }

        var name = Path.GetFileName(path);
        if (name.StartsWith('.'))
        {
            return found;
        }

        FileAttributes attr;
        try
        {
            attr = File.GetAttributes(path);
        }
        catch
        {
            return found;
        }

        if ((attr & FileAttributes.Hidden) != 0 || (attr & FileAttributes.ReparsePoint) != 0)
        {
            return found;
        }

        if ((attr & FileAttributes.Directory) != 0)
        {
            Walk(path, found, 0);
            return found;
        }

        found.Add(path);
        return found;
    }

    static void Walk(string dir, List<string> found, int depth)
    {
        if (depth > 32 || found.Count >= 500)
        {
            return;
        }

        IEnumerable<string> entries;
        try
        {
            entries = Directory.EnumerateFileSystemEntries(dir);
        }
        catch
        {
            return;
        }

        foreach (var entry in entries)
        {
            if (found.Count >= 500)
            {
                return;
            }

            var entryName = Path.GetFileName(entry);
            if (entryName.StartsWith('.'))
            {
                continue;
            }

            FileAttributes entryAttr;
            try
            {
                entryAttr = File.GetAttributes(entry);
            }
            catch
            {
                continue;
            }

            if ((entryAttr & FileAttributes.Hidden) != 0 || (entryAttr & FileAttributes.ReparsePoint) != 0)
            {
                continue;
            }

            if ((entryAttr & FileAttributes.Directory) != 0)
            {
                Walk(entry, found, depth + 1);
            }
            else
            {
                found.Add(entry);
            }
        }
    }
}
