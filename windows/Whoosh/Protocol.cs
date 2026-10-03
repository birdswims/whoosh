using System.Text.Json.Serialization;

namespace Whoosh;

public sealed class Command
{
    [JsonPropertyName("id")]
    public string Id { get; set; } = "";

    [JsonPropertyName("op")]
    public string Op { get; set; } = "";

    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("dir")]
    public string? Dir { get; set; }

    [JsonPropertyName("native")]
    public bool? Native { get; set; }

    [JsonPropertyName("quickshare")]
    public bool? Quickshare { get; set; }

    [JsonPropertyName("airdrop")]
    public bool? Airdrop { get; set; }

    [JsonPropertyName("sort_media")]
    public bool? SortMedia { get; set; }

    [JsonPropertyName("require_pin")]
    public bool? RequirePin { get; set; }

    [JsonPropertyName("enabled")]
    public bool? Enabled { get; set; }

    [JsonPropertyName("via")]
    public string? Via { get; set; }

    [JsonPropertyName("target")]
    public string? Target { get; set; }

    [JsonPropertyName("files")]
    public List<string>? Files { get; set; }

    [JsonPropertyName("pin")]
    public string? Pin { get; set; }

    [JsonPropertyName("trust")]
    public bool? Trust { get; set; }

    [JsonPropertyName("fingerprint")]
    public string? Fingerprint { get; set; }

    [JsonPropertyName("accept")]
    public bool? Accept { get; set; }

    [JsonPropertyName("offer")]
    public string? Offer { get; set; }

    [JsonPropertyName("peer_name")]
    public string? PeerName { get; set; }
}

public sealed class Peer
{
    [JsonPropertyName("id")]
    public string Id { get; set; } = "";

    [JsonPropertyName("via")]
    public string Via { get; set; } = "";

    [JsonPropertyName("name")]
    public string Name { get; set; } = "";

    [JsonPropertyName("detail")]
    public string Detail { get; set; } = "";

    [JsonPropertyName("address")]
    public string Address { get; set; } = "";

    [JsonPropertyName("fingerprint")]
    public string? Fingerprint { get; set; }

    [JsonPropertyName("trusted")]
    public bool Trusted { get; set; }

    public string ProtocolLabel => Format.Protocol(Via);
}

public sealed class OfferFile
{
    [JsonPropertyName("name")]
    public string Name { get; set; } = "";

    [JsonPropertyName("bytes")]
    public ulong Bytes { get; set; }

    [JsonPropertyName("mime")]
    public string Mime { get; set; } = "";

    [JsonPropertyName("kind")]
    public string Kind { get; set; } = "";

    public string SizeLabel => Bytes == 0 ? "Size unknown" : Format.Size(Bytes);

    public string Glyph => Format.Glyph(Name, Kind);
}

public sealed class WireEvent
{
    [JsonPropertyName("ev")]
    public string Ev { get; set; } = "";

    [JsonPropertyName("id")]
    public string? Id { get; set; }

    [JsonPropertyName("ok")]
    public bool? Ok { get; set; }

    [JsonPropertyName("error")]
    public string? Error { get; set; }

    [JsonPropertyName("version")]
    public string? Version { get; set; }

    [JsonPropertyName("device")]
    public string? Device { get; set; }

    [JsonPropertyName("fingerprint")]
    public string? Fingerprint { get; set; }

    [JsonPropertyName("receiving")]
    public bool? Receiving { get; set; }

    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("dir")]
    public string? Dir { get; set; }

    [JsonPropertyName("pin")]
    public string? Pin { get; set; }

    [JsonPropertyName("native")]
    public bool? Native { get; set; }

    [JsonPropertyName("quickshare")]
    public bool? Quickshare { get; set; }

    [JsonPropertyName("airdrop")]
    public bool? Airdrop { get; set; }

    [JsonPropertyName("sort_media")]
    public bool? SortMedia { get; set; }

    [JsonPropertyName("require_pin")]
    public bool? RequirePin { get; set; }

    [JsonPropertyName("visibility")]
    public string? Visibility { get; set; }

    [JsonPropertyName("address")]
    public string? Address { get; set; }

    [JsonPropertyName("warnings")]
    public List<string>? Warnings { get; set; }

    [JsonPropertyName("peers")]
    public List<Peer>? Peers { get; set; }

    [JsonPropertyName("via")]
    public string? Via { get; set; }

    [JsonPropertyName("peer")]
    public string? Peer { get; set; }

    [JsonPropertyName("files")]
    public List<OfferFile>? Files { get; set; }

    [JsonPropertyName("direction")]
    public string? Direction { get; set; }

    [JsonPropertyName("state")]
    public string? State { get; set; }

    [JsonPropertyName("title")]
    public string? Title { get; set; }

    [JsonPropertyName("detail")]
    public string? Detail { get; set; }

    [JsonPropertyName("bytes")]
    public ulong? Bytes { get; set; }

    [JsonPropertyName("paths")]
    public List<string>? Paths { get; set; }
}

public sealed class Offer
{
    public required string Id { get; init; }
    public required string Via { get; init; }
    public required string Peer { get; init; }
    public string? Pin { get; init; }
    public required IReadOnlyList<OfferFile> Files { get; init; }

    public ulong TotalBytes => Files.Aggregate(0UL, (sum, file) => sum + file.Bytes);

    public bool SizeUnknown => Files.Count > 0 && Files.All(file => file.Bytes == 0);
}
