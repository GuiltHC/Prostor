using System.Collections.ObjectModel;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;

namespace Prostor.ViewModels;

public sealed class DashboardViewModel : ObservableObject
{
    private readonly MainViewModel _main;

    public ObservableCollection<DriveItem> Drives { get; } = new();
    public ObservableCollection<Recommendation> Recommendations { get; } = new();

    public ICommand RefreshCommand { get; }
    public ICommand AnalyzeDriveCommand { get; }
    public ICommand GoCommand { get; }
    public ICommand OpenAppCommand { get; }
    public ICommand RunRecommendationCommand { get; }

    public DashboardViewModel(MainViewModel main)
    {
        _main = main;
        RefreshCommand = new RelayCommand(async () =>
        {
            Refresh();
            await Task.WhenAll(_main.Programs.LoadAsync(), _main.Cleanup.ScanAsync(), _main.Startup.LoadAsync());
        });
        AnalyzeDriveCommand = new RelayCommand(p =>
        {
            if (p is DriveItem d) _main.Analyzer.StartScan(d.RootPath);
        });
        GoCommand = new RelayCommand(p => _main.Navigate(p as string));
        OpenAppCommand = new RelayCommand(p =>
        {
            if (p is BarItem { App: { } app })
            {
                _main.Programs.SelectApp(app);
                _main.Navigate("programs");
            }
        });
        RunRecommendationCommand = new RelayCommand(p => (p as Recommendation)?.Action?.Invoke());

        main.Programs.Changed += RefreshStats;
        main.Cleanup.Changed += RefreshStats;
        main.Startup.Changed += RefreshStats;
    }

    public string Greeting
    {
        get
        {
            int h = DateTime.Now.Hour;
            string g = h >= 5 && h < 12 ? "Доброе утро" : h >= 12 && h < 18 ? "Добрый день" : h >= 18 && h < 23 ? "Добрый вечер" : "Доброй ночи";
            return $"{g}, {Environment.UserName}";
        }
    }

    public string SystemText => $"{ShellHelper.WindowsVersionText()} · {Environment.MachineName}";

    // ---- tiles (animated numbers: NaN shows a dash)
    private double _programsValue = double.NaN;
    public double ProgramsValue { get => _programsValue; private set => Set(ref _programsValue, value); }

    private string _programsSize = "";
    public string ProgramsSize { get => _programsSize; private set => Set(ref _programsSize, value); }

    private double _junkValue = double.NaN;
    public double JunkValue { get => _junkValue; private set => Set(ref _junkValue, value); }

    private string _junkNote = "";
    public string JunkNote { get => _junkNote; private set => Set(ref _junkNote, value); }

    private double _startupValue = double.NaN;
    public double StartupValue { get => _startupValue; private set => Set(ref _startupValue, value); }

    private string _startupNote = "";
    public string StartupNote { get => _startupNote; private set => Set(ref _startupNote, value); }

    private List<BarItem> _topApps = new();
    public List<BarItem> TopApps { get => _topApps; private set => Set(ref _topApps, value); }

    private List<BarItem> _recentApps = new();
    public List<BarItem> RecentApps { get => _recentApps; private set => Set(ref _recentApps, value); }

    private string _diskTip;
    public string DiskTip { get => _diskTip; private set => Set(ref _diskTip, value); }

    // ---- recommendations
    private double _recommendedValue = double.NaN;
    /// <summary>Space all recommendations together would free.</summary>
    public double RecommendedValue { get => _recommendedValue; private set => Set(ref _recommendedValue, value); }

    public bool HasRecommendations => Recommendations.Count > 0;

    private (int count, long size) _oldDownloads;

    public void Refresh()
    {
        OnPropertyChanged(nameof(Greeting));
        Drives.Clear();
        foreach (var d in DriveService.GetDrives()) Drives.Add(d);
        var low = Drives.FirstOrDefault(d => d.Level != "ok");
        DiskTip = low == null
            ? null
            : $"На диске {low.Name} осталось всего {Fmt.Size(low.Free)} ({Math.Round((1 - low.UsedFraction) * 100)}%). Начните с рекомендаций ниже.";
        RefreshHistory();
        _ = LoadHealthAsync();
        _ = MeasureDownloadsAsync();
        RefreshStats();
    }

    /// <summary>Free space chart and the "disk will be full in N days" forecast.</summary>
    public void RefreshHistory()
    {
        var history = DiskHistory.Load();
        foreach (var d in Drives)
        {
            if (!history.TryGetValue(d.Name, out var points) || points.Count == 0)
            {
                d.History = null;
                d.ForecastText = "История появится после нескольких дней использования";
                continue;
            }
            var recent = points.Where(p => p.Date >= DateTime.Now.AddDays(-90)).ToList();
            d.History = recent.Count >= 2 ? recent.Select(p => p.Free / (double)(1L << 30)).ToList() : null;
            var f = DiskHistory.Forecast(points);
            if (f == null)
            {
                d.ForecastText = "Прогноз появится через несколько дней";
                d.ForecastLevel = "ok";
            }
            else if (f.DaysUntilFull is { } days)
            {
                d.ForecastText = $"Теряет ≈ {Fmt.Size((long)-f.BytesPerDay)} в день — заполнится примерно через {Fmt.Count(days, "день", "дня", "дней")}";
                d.ForecastLevel = days < 30 ? "danger" : days < 120 ? "warn" : "ok";
            }
            else
            {
                d.ForecastText = f.BytesPerDay > 0
                    ? $"Свободного места становится больше · за {Fmt.Count(f.Days, "день", "дня", "дней")}"
                    : $"Свободное место почти не меняется · за {Fmt.Count(f.Days, "день", "дня", "дней")}";
                d.ForecastLevel = "ok";
            }
        }
        RefreshStats();
    }

    private async Task LoadHealthAsync()
    {
        var disks = await Task.Run(DiskHealthService.Load);
        foreach (var d in Drives)
        {
            var disk = disks.FirstOrDefault(x => d.Name.Length > 0 && x.Letters.Contains(char.ToUpperInvariant(d.Name[0])));
            if (disk == null) continue;
            d.HealthText = $"{disk.MediaType} · {disk.Status}";
            d.DiskModel = string.IsNullOrEmpty(disk.Model) ? disk.Summary : $"{disk.Model}\n{disk.Summary}";
            d.HealthLevel = disk.Level == "unknown" ? null : disk.Level;
        }
        RefreshStats();
    }

    /// <summary>Big files lying in Downloads for more than three months.</summary>
    private async Task MeasureDownloadsAsync()
    {
        _oldDownloads = await Task.Run(() =>
        {
            string dl = ShellHelper.DownloadsFolder;
            if (!Directory.Exists(dl)) return (0, 0L);
            var cutoff = DateTime.Now.AddDays(-90);
            int count = 0;
            long size = 0;
            var options = new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.ReparsePoint };
            try
            {
                foreach (var f in new DirectoryInfo(dl).EnumerateFiles("*", options))
                {
                    if (f.Length < 10L << 20) continue;
                    var arrived = f.CreationTime < f.LastWriteTime ? f.CreationTime : f.LastWriteTime;
                    if (arrived >= cutoff) continue;
                    count++;
                    size += f.Length;
                }
            }
            catch { }
            return (count, size);
        });
        RefreshStats();
    }

    public void RefreshStats()
    {
        var p = _main.Programs;
        if (p.Apps.Count > 0)
        {
            ProgramsValue = p.Apps.Count;
            ProgramsSize = $"{Fmt.Plural(p.Apps.Count, "программа", "программы", "программ")} · ≈ {Fmt.Size(p.TotalSize)}";
            var top = p.Apps.Where(a => a.SizeBytes > 0).OrderByDescending(a => a.SizeBytes).Take(6).ToList();
            double max = top.Count > 0 ? top[0].SizeBytes : 1;
            TopApps = top.Select(a => new BarItem { App = a, Title = a.Name, Value = a.SizeText, Fraction = a.SizeBytes / max }).ToList();
            RecentApps = p.Apps.Where(a => a.InstallDate.HasValue).OrderByDescending(a => a.InstallDate).Take(6)
                .Select(a => new BarItem { App = a, Title = a.Name, Value = a.AgoText }).ToList();
        }
        else
        {
            ProgramsValue = double.NaN;
            ProgramsSize = p.IsLoading ? "загружаем…" : "";
        }

        var c = _main.Cleanup;
        JunkValue = c.IsScanning && c.TotalFound == 0 ? double.NaN : c.TotalFound;
        JunkNote = c.IsScanning ? "анализируем…" : "временные файлы и кэши";

        var s = _main.Startup;
        StartupValue = s.Items.Count == 0 ? double.NaN : s.EnabledCount;
        StartupNote = s.Items.Count == 0 ? "" : $"из {s.Items.Count} включены при старте";

        BuildRecommendations();
    }

    private void BuildRecommendations()
    {
        var list = new List<Recommendation>();
        var cleanup = _main.Cleanup;
        var programs = _main.Programs;

        foreach (var d in Drives.Where(d => d.ForecastLevel is "danger" or "warn"))
            list.Add(new Recommendation
            {
                Glyph = "", Tone = d.ForecastLevel == "danger" ? "danger" : "warning",
                Title = $"Диск {d.Name} заполняется", Detail = d.ForecastText, ActionText = "Найти, что растёт",
                Action = () => _main.Analyzer.StartScan(d.RootPath)
            });
        foreach (var d in Drives.Where(d => d.HealthLevel is "danger" or "warn"))
            list.Add(new Recommendation
            {
                Glyph = "", Tone = d.HealthLevel == "danger" ? "danger" : "warning",
                Title = $"Диск {d.Name}: {d.HealthText}", Detail = $"{d.DiskModel}. Сделайте резервную копию важных данных."
            });

        if (cleanup.TotalFound >= 100L << 20)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "success", Title = "Временные файлы и кэши", Size = cleanup.TotalFound,
                Detail = "Их можно удалить без последствий — программы создадут их заново", ActionText = "Очистить",
                Action = () => _main.Navigate("cleanup")
            });

        var unused = programs.Apps.Where(a => a.IsUnused && !a.IsGame).ToList();
        if (unused.Count > 0)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "info", Title = $"{Fmt.Programs(unused.Count)} давно не запускались",
                Size = unused.Where(a => a.SizeBytes > 0).Sum(a => a.SizeBytes),
                Detail = string.Join(", ", unused.OrderByDescending(a => a.SizeBytes).Take(3).Select(a => a.Name)) + (unused.Count > 3 ? " и другие" : ""),
                ActionText = "Посмотреть",
                Action = () =>
                {
                    programs.Filter = AppFilter.Unused;
                    _main.Navigate("programs");
                }
            });

        var games = programs.Apps.Where(a => a.IsGame && a.LastUsed is { } t && t < DateTime.Now.AddMonths(-4)).ToList();
        if (games.Count > 0)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "info", Title = $"{Fmt.Count(games.Count, "игра", "игры", "игр")} без запуска больше 4 месяцев",
                Size = games.Where(a => a.SizeBytes > 0).Sum(a => a.SizeBytes),
                Detail = string.Join(", ", games.OrderByDescending(a => a.SizeBytes).Take(3).Select(a => a.Name)),
                ActionText = "Посмотреть",
                Action = () =>
                {
                    programs.Filter = AppFilter.Games;
                    programs.Sort = AppSort.LastUsed;
                    _main.Navigate("programs");
                }
            });

        if (_oldDownloads.count > 0 && _oldDownloads.size >= 100L << 20)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "info", Title = "Старые файлы в «Загрузках»", Size = _oldDownloads.size,
                Detail = $"{Fmt.Files(_oldDownloads.count)} крупнее 10 МБ лежат там дольше трёх месяцев — обычно это ненужные установщики и архивы",
                ActionText = "Разобрать",
                Action = () => _main.Analyzer.StartScan(ShellHelper.DownloadsFolder, "files", "downloads")
            });

        if (cleanup.WindowsOldSize > 0)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "info", Title = "Папка Windows.old", Size = cleanup.WindowsOldSize,
                Detail = "Копия прошлой версии Windows. Удаляется в «Параметры → Система → Память»", ActionText = "Открыть",
                Action = () => ShellHelper.Open("ms-settings:storagesense")
            });

        if (programs.CountUpdates > 0)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "accent", Title = $"Доступны обновления: {Fmt.Programs(programs.CountUpdates)}",
                Detail = "Новые версии исправляют ошибки и уязвимости", ActionText = "Обновить",
                Action = () =>
                {
                    programs.Filter = AppFilter.Updates;
                    _main.Navigate("programs");
                }
            });

        var startup = _main.Startup;
        if (startup.EnabledCount >= 12)
            list.Add(new Recommendation
            {
                Glyph = "", Tone = "warning", Title = $"{startup.EnabledCount} программ запускаются вместе с Windows",
                Detail = "Отключите лишние или запускайте их через минуту — компьютер будет загружаться быстрее", ActionText = "Настроить",
                Action = () => _main.Navigate("startup")
            });

        Recommendations.Clear();
        foreach (var r in list) Recommendations.Add(r);
        long total = list.Sum(r => r.Size);
        RecommendedValue = total > 0 ? total : double.NaN;
        OnPropertyChanged(nameof(HasRecommendations));
    }
}
