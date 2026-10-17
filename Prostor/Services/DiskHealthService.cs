using System.Management;

namespace Prostor.Services;

public sealed class DiskHealth
{
    public int DiskNumber { get; init; }
    public string Model { get; init; }
    public string MediaType { get; init; }   // "SSD", "HDD", "NVMe SSD"…
    /// <summary>"ok", "warn", "danger" or "unknown".</summary>
    public string Level { get; init; }
    public string Status { get; init; }
    public int? Temperature { get; init; }
    /// <summary>Percent of rated endurance used (SSD).</summary>
    public int? Wear { get; init; }
    public long? PowerOnHours { get; init; }
    public List<char> Letters { get; init; } = new();

    public string Summary
    {
        get
        {
            var parts = new List<string> { Status };
            if (Wear is { } w) parts.Add($"износ {w}%");
            if (Temperature is { } t and > 0) parts.Add($"{t} °C");
            if (PowerOnHours is { } h and > 0) parts.Add($"{h / 24:N0} дн. в работе");
            return string.Join(" · ", parts);
        }
    }
}

/// <summary>State of physical disks from the Windows storage subsystem (SMART-based health, wear and temperature).</summary>
public static class DiskHealthService
{
    private const string Ns = @"\\.\root\Microsoft\Windows\Storage";

    public static List<DiskHealth> Load()
    {
        var result = new List<DiskHealth>();
        try
        {
            var scope = new ManagementScope(Ns);
            scope.Connect();

            var letters = new Dictionary<int, List<char>>();
            using (var parts = new ManagementObjectSearcher(scope, new ObjectQuery("SELECT DiskNumber, DriveLetter FROM MSFT_Partition")))
            {
                foreach (ManagementObject p in parts.Get())
                {
                    char letter = p["DriveLetter"] is char c ? c : p["DriveLetter"] is ushort u ? (char)u : '\0';
                    if (letter == '\0' || !char.IsLetter(letter)) continue;
                    int disk = Convert.ToInt32(p["DiskNumber"]);
                    if (!letters.TryGetValue(disk, out var list)) letters[disk] = list = new List<char>();
                    list.Add(char.ToUpperInvariant(letter));
                }
            }

            using var disks = new ManagementObjectSearcher(scope, new ObjectQuery("SELECT * FROM MSFT_PhysicalDisk"));
            foreach (ManagementObject d in disks.Get())
            {
                int number = int.TryParse(d["DeviceId"]?.ToString(), out int n) ? n : -1;
                int media = Convert.ToInt32(d["MediaType"] ?? 0);
                int bus = Convert.ToInt32(d["BusType"] ?? 0);
                int health = Convert.ToInt32(d["HealthStatus"] ?? 5);
                string type = media switch
                {
                    4 => bus == 17 ? "NVMe SSD" : "SSD",
                    3 => "HDD",
                    _ => bus == 17 ? "NVMe" : bus == 7 ? "USB" : "Диск"
                };
                int? temp = null, wear = null;
                long? hours = null;
                try
                {
                    foreach (ManagementObject r in d.GetRelated("MSFT_StorageReliabilityCounter"))
                    {
                        temp = ToInt(r["Temperature"]);
                        wear = ToInt(r["Wear"]);
                        hours = r["PowerOnHours"] is { } h ? Convert.ToInt64(h) : null;
                    }
                }
                catch { } // needs administrator rights

                string level = health switch { 0 => "ok", 1 => "warn", 2 => "danger", _ => "unknown" };
                if (level == "ok" && wear >= 90) level = "warn";
                if (level == "ok" && temp >= 70) level = "warn";
                result.Add(new DiskHealth
                {
                    DiskNumber = number,
                    Model = (d["FriendlyName"] as string)?.Trim(),
                    MediaType = type,
                    Level = level,
                    Status = level switch
                    {
                        "ok" => "Исправен",
                        "warn" => wear >= 90 ? "Сильно изношен" : temp >= 70 ? "Перегрев" : "Есть предупреждения",
                        "danger" => "Неисправен — сделайте копию данных",
                        _ => "Состояние неизвестно"
                    },
                    Temperature = temp,
                    Wear = wear,
                    PowerOnHours = hours,
                    Letters = letters.TryGetValue(number, out var l) ? l : new List<char>()
                });
            }
        }
        catch { }
        return result;
    }

    private static int? ToInt(object o)
    {
        if (o == null) return null;
        try
        {
            int v = Convert.ToInt32(o);
            return v;
        }
        catch
        {
            return null;
        }
    }
}
