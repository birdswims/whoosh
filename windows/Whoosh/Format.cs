namespace Whoosh;

public static class Format
{
    public static string Protocol(string? via) => via switch
    {
        "whoosh" => "Whoosh",
        "quickshare" => "Quick Share",
        "airdrop" => "AirDrop",
        _ => "Device",
    };

    public static string Size(ulong bytes)
    {
        string[] units = ["B", "KB", "MB", "GB", "TB"];
        double value = bytes;
        var unit = 0;
        while (value >= 1024 && unit < units.Length - 1)
        {
            value /= 1024;
            unit++;
        }

        return unit == 0 ? $"{bytes} B" : $"{value:0.0} {units[unit]}";
    }

    public static string Fingerprint(string hex)
    {
        var clean = new string(hex.ToLowerInvariant().Where(ch => !char.IsWhiteSpace(ch)).ToArray());
        var groups = new List<string>();
        for (var index = 0; index < clean.Length; index += 4)
        {
            groups.Add(clean.Substring(index, Math.Min(4, clean.Length - index)));
        }

        var lines = new List<string>();
        for (var index = 0; index < groups.Count; index += 8)
        {
            lines.Add(string.Join(' ', groups.Skip(index).Take(8)));
        }

        return string.Join('\n', lines);
    }

    public static string Abbreviate(string path)
    {
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile).TrimEnd('\\');
        if (string.Equals(path, home, StringComparison.OrdinalIgnoreCase))
        {
            return "~";
        }

        var prefix = home + "\\";
        if (path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
        {
            return "~\\" + path[prefix.Length..];
        }

        return path;
    }

    public static bool IsImage(string path)
    {
        var ext = System.IO.Path.GetExtension(path).ToLowerInvariant();
        return ext is ".jpg" or ".jpeg" or ".png" or ".gif" or ".bmp" or ".webp" or ".tif" or ".tiff" or ".heic" or ".heif" or ".avif" or ".jfif";
    }

    public static string Glyph(string fileName, string? kind)
    {
        switch (kind)
        {
            case "photo": return "\uE91B";
            case "video": return "\uE714";
            case "audio": return "\uE8D6";
        }

        var ext = System.IO.Path.GetExtension(fileName).ToLowerInvariant();
        return ext switch
        {
            ".jpg" or ".jpeg" or ".png" or ".gif" or ".heic" or ".heif" or ".webp" or ".tif" or ".tiff" or ".avif" or ".bmp" or ".jfif" => "\uE91B",
            ".mp4" or ".mov" or ".m4v" or ".webm" or ".mkv" or ".avi" or ".wmv" => "\uE714",
            ".mp3" or ".m4a" or ".wav" or ".flac" or ".aac" or ".aiff" => "\uE8D6",
            _ => "\uE8A5",
        };
    }
}
