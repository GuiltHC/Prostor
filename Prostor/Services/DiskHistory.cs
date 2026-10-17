using System.Text.Json;
using Prostor.Core;

namespace Prostor.Services;

public sealed class DiskPoint
{
    public DateTime Date { get; set; }
    public long Free { get; set; }
    public long Total { get; set; }
}

public sealed record DiskForecast(double BytesPerDay, int? DaysUntilFull, int Days, int Points);

/// <summary>Free space of each drive, one point per day (recorded whenever the app runs).</summary>
public static class DiskHistory
{
    private const int KeepDays = 400;
    private static readonly object Lock = new();

    private static string FilePath => Path.Combine(AppSettings.DataFolder, "disk-history.json");

    public static Dictionary<string, List<DiskPoint>> Load()
    {
        lock (Lock)
        {
            try
            {
                if (File.Exists(FilePath))
                    return JsonSerializer.Deserialize<Dictionary<string, List<DiskPoint>>>(File.ReadAllText(FilePath))
                           ?? new();
            }
            catch { }
            return new();
        }
    }

    public static void Record()
    {
        lock (Lock)
        {
            var data = Load();
            var today = DateTime.Today;
            foreach (var d in DriveInfo.GetDrives())
            {
                try
                {
                    if (d.DriveType != DriveType.Fixed || !d.IsReady) continue;
                    string key = d.Name.TrimEnd('\\');
                    if (!data.TryGetValue(key, out var list)) data[key] = list = new List<DiskPoint>();
                    list.RemoveAll(p => p.Date.Date == today || p.Date < today.AddDays(-KeepDays));
                    list.Add(new DiskPoint { Date = DateTime.Now, Free = d.AvailableFreeSpace, Total = d.TotalSize });
                    list.Sort((a, b) => a.Date.CompareTo(b.Date));
                }
                catch { }
            }
            try
            {
                Directory.CreateDirectory(AppSettings.DataFolder);
                File.WriteAllText(FilePath, JsonSerializer.Serialize(data));
            }
            catch { }
        }
    }

    /// <summary>Linear trend of free space over the last <paramref name="windowDays"/> days.</summary>
    public static DiskForecast Forecast(IReadOnlyList<DiskPoint> points, int windowDays = 90)
    {
        if (points == null || points.Count < 3) return null;
        var last = points[^1].Date;
        var window = points.Where(p => p.Date >= last.AddDays(-windowDays)).ToList();
        if (window.Count < 3) return null;
        double span = (window[^1].Date - window[0].Date).TotalDays;
        if (span < 3) return null;

        double t0 = window[0].Date.Ticks;
        var xs = window.Select(p => (p.Date.Ticks - t0) / TimeSpan.TicksPerDay).ToArray();
        var ys = window.Select(p => (double)p.Free).ToArray();
        double mx = xs.Average(), my = ys.Average();
        double sxx = xs.Sum(x => (x - mx) * (x - mx));
        if (sxx <= 0) return null;
        double slope = xs.Zip(ys, (x, y) => (x - mx) * (y - my)).Sum() / sxx;

        int? days = null;
        if (slope < -1)
        {
            double remaining = window[^1].Free / -slope;
            if (remaining < 3650) days = (int)Math.Round(remaining);
        }
        return new DiskForecast(slope, days, (int)Math.Round(span), window.Count);
    }
}
