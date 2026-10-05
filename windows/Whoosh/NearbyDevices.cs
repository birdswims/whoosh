using System.Net;

namespace Whoosh;

/// <summary>
/// One nearby row per computer. Whoosh wins when that computer runs it.
/// Quick Share is next, then AirDrop. The other protocols stay available
/// in the hidden list.
/// </summary>
static class NearbyDevices
{
    public static (List<Peer> Primary, List<Peer> Hidden) Split(IReadOnlyList<Peer> peers)
    {
        var count = peers.Count;
        var parent = new int[count];
        for (var index = 0; index < count; index++)
        {
            parent[index] = index;
        }

        var hosts = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var names = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        for (var index = 0; index < count; index++)
        {
            var peer = peers[index];
            var host = HostKey(peer.Address);
            if (host != null)
            {
                if (hosts.TryGetValue(host, out var other))
                {
                    Union(parent, index, other);
                }
                else
                {
                    hosts[host] = index;
                }
            }

            if (!Distinctive(peer.Name))
            {
                continue;
            }

            var name = peer.Name.Trim();
            if (!names.TryGetValue(name, out var named))
            {
                names[name] = index;
                continue;
            }

            // Two Whoosh computers can share a display name. Only fold a
            // different protocol into that name.
            if (!string.Equals(peers[named].Via, peer.Via, StringComparison.OrdinalIgnoreCase))
            {
                Union(parent, index, named);
            }
        }

        var groups = new Dictionary<int, List<int>>();
        for (var index = 0; index < count; index++)
        {
            var root = Find(parent, index);
            if (!groups.TryGetValue(root, out var group))
            {
                group = [];
                groups[root] = group;
            }

            group.Add(index);
        }

        var hidden = new HashSet<int>();
        foreach (var group in groups.Values)
        {
            if (group.Count < 2)
            {
                continue;
            }

            var best = group[0];
            foreach (var index in group)
            {
                if (Prefer(peers[index], peers[best]) < 0)
                {
                    best = index;
                }
            }

            foreach (var index in group)
            {
                if (index != best)
                {
                    hidden.Add(index);
                }
            }
        }

        var primary = new List<Peer>();
        var extras = new List<Peer>();
        for (var index = 0; index < count; index++)
        {
            if (hidden.Contains(index))
            {
                extras.Add(peers[index]);
            }
            else
            {
                primary.Add(peers[index]);
            }
        }

        return (primary, extras);
    }

    public static Peer? PreferredVisible(Peer extra, IReadOnlyList<Peer> primary)
    {
        var host = HostKey(extra.Address);
        if (host != null)
        {
            foreach (var peer in primary)
            {
                if (string.Equals(HostKey(peer.Address), host, StringComparison.OrdinalIgnoreCase))
                {
                    return peer;
                }
            }
        }

        if (!Distinctive(extra.Name))
        {
            return null;
        }

        foreach (var peer in primary)
        {
            if (Distinctive(peer.Name)
                && string.Equals(peer.Name.Trim(), extra.Name.Trim(), StringComparison.OrdinalIgnoreCase)
                && !string.Equals(peer.Via, extra.Via, StringComparison.OrdinalIgnoreCase))
            {
                return peer;
            }
        }

        return null;
    }

    static int Prefer(Peer candidate, Peer current)
    {
        var rank = Rank(candidate.Via).CompareTo(Rank(current.Via));
        if (rank != 0)
        {
            return rank;
        }

        var name = string.Compare(candidate.Name, current.Name, StringComparison.OrdinalIgnoreCase);
        if (name != 0)
        {
            return name;
        }

        return string.Compare(candidate.Id, current.Id, StringComparison.Ordinal);
    }

    static int Rank(string via) => via switch
    {
        "whoosh" => 0,
        "quickshare" => 1,
        "airdrop" => 2,
        _ => 3,
    };

    static bool Distinctive(string? name)
    {
        if (string.IsNullOrWhiteSpace(name))
        {
            return false;
        }

        var trimmed = name.Trim();
        return !trimmed.Equals("Quick Share device", StringComparison.OrdinalIgnoreCase)
            && !trimmed.Equals("AirDrop device", StringComparison.OrdinalIgnoreCase)
            && !trimmed.Equals("Device", StringComparison.OrdinalIgnoreCase);
    }

    static string? HostKey(string? address)
    {
        if (string.IsNullOrWhiteSpace(address))
        {
            return null;
        }

        if (!IPEndPoint.TryParse(address.Trim(), out var endpoint))
        {
            return null;
        }

        var ip = endpoint.Address;
        if (ip.IsIPv4MappedToIPv6)
        {
            ip = ip.MapToIPv4();
        }

        if (ip.AddressFamily == System.Net.Sockets.AddressFamily.InterNetworkV6 && ip.ScopeId != 0)
        {
            return ip + "%" + ip.ScopeId;
        }

        return ip.ToString();
    }

    static int Find(int[] parent, int index)
    {
        while (parent[index] != index)
        {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }

        return index;
    }

    static void Union(int[] parent, int left, int right)
    {
        var a = Find(parent, left);
        var b = Find(parent, right);
        if (a != b)
        {
            parent[b] = a;
        }
    }
}
