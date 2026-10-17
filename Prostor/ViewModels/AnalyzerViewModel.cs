using System.Collections.ObjectModel;
using System.Diagnostics;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;
using Microsoft.Win32;
using Prostor.Controls;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;

namespace Prostor.ViewModels;

public enum AnalyzerState { Idle, Scanning, Done }

public sealed class Crumb
{
    public string Name { get; init; }
    public DirNode Node { get; init; }
    public bool IsFirst { get; init; }
}

public sealed class QuickFolder
{
    public string Title { get; init; }
    public string Path { get; init; }
    public string Glyph { get; init; }
}

/// <summary>Row in the folder list (a sub-folder, a file or an aggregate).</summary>
public sealed class NodeItem
{
    private static readonly SolidColorBrush UnknownBrush = Frozen(Color.FromRgb(0x7A, 0x84, 0x99));

    public string Name { get; init; }
    public long Size { get; init; }
    public double Fraction { get; init; }
    public DirNode Dir { get; init; }
    public DirNode Parent { get; init; }
    public string FileName { get; init; }
    public long LastWrite { get; init; }
    public byte Category { get; init; }
    public bool IsRest { get; init; }
    public long RestCount { get; init; }

    public bool IsDir => Dir != null;
    public bool IsVirtual => Dir?.IsVirtual == true || IsRest;
    public bool CanAct => !IsVirtual;

    public string SizeText => Fmt.Size(Size);
    public string PercentText => Fmt.Percent(Fraction);
    public string FullPath => IsDir ? Dir.FullPath : Path.Combine(Parent.FullPath, FileName);
    public object TreemapKey => IsDir ? Dir : IsRest ? null : new FileRef(Parent, FileName);

    public string Glyph => Dir?.IsVirtual == true ? "" : IsRest ? "" : IsDir ? "" : FileCategories.Get(Category).Glyph;

    public Brush GlyphBrush => Dir?.IsVirtual == true || IsRest
        ? UnknownBrush
        : IsDir ? (System.Windows.Application.Current.TryFindResource("Brush.Folder") as Brush ?? Brushes.Goldenrod)
        : FileCategories.Get(Category).Brush;

    public string Details
    {
        get
        {
            if (Dir?.IsVirtual == true) return "Теневые копии, метаданные и файлы без доступа";
            if (IsRest) return Fmt.Files(RestCount);
            if (IsDir)
            {
                string s = Fmt.Files(Dir.FileCount);
                if (Dir.AccessDenied) s += " · нет доступа";
                return s;
            }
            return LastWrite > 0 ? Fmt.Date(DateTime.FromFileTime(LastWrite)) : FileCategories.Get(Category).Title;
        }
    }

    private static SolidColorBrush Frozen(Color c)
    {
        var b = new SolidColorBrush(c);
        b.Freeze();
        return b;
    }
}

public sealed class LegendItem : ObservableObject
{
    public CategoryInfo Info { get; init; }
    public string Title => Info.Title;
    public Brush Brush => Info.Brush;

    private bool _isDimmed;
    public bool IsDimmed { get => _isDimmed; set => Set(ref _isDimmed, value); }
}

public sealed class AnalyzerViewModel : ObservableObject
{
    private const int MaxFileRows = 2000;
    private readonly MainViewModel _main;
    private CancellationTokenSource _cts;
    private object _scanner;
    private DispatcherTimer _timer;
    private Stopwatch _watch;
    private long _expectedBytes;
    private string _scanPath;

    public ObservableCollection<DriveItem> Drives { get; } = new();
    public ObservableCollection<QuickFolder> QuickFolders { get; } = new();
    public ObservableCollection<Crumb> Breadcrumbs { get; } = new();

    public ICommand ScanDriveCommand { get; }
    public ICommand ScanPathCommand { get; }
    public ICommand BrowseCommand { get; }
    public ICommand CancelCommand { get; }
    public ICommand NewScanCommand { get; }
    public ICommand RescanCommand { get; }
    public ICommand UpCommand { get; }
    public ICommand CrumbCommand { get; }
    public ICommand OpenItemCommand { get; }
    public ICommand ShowInExplorerCommand { get; }
    public ICommand CopyPathCommand { get; }
    public ICommand DeleteCommand { get; }
    public ICommand OpenLargeFileFolderCommand { get; }
    public ICommand DeleteLargeFileCommand { get; }
    public ICommand CompressCommand { get; }
    public ICommand MoveCommand { get; }
    public ICommand ExcludeCommand { get; }
    public ICommand OpenChangeCommand { get; }
    public ICommand ToggleHighlightCommand { get; }
    public ICommand FindDuplicatesCommand { get; }
    public ICommand CancelDuplicatesCommand { get; }
    public ICommand KeepNewestCommand { get; }
    public ICommand KeepOldestCommand { get; }
    public ICommand ClearDupMarksCommand { get; }
    public ICommand DeleteDuplicatesCommand { get; }
    public ICommand ShowDuplicateCommand { get; }

    public AnalyzerViewModel(MainViewModel main)
    {
        _main = main;
        ScanDriveCommand = new RelayCommand(p =>
        {
            if (p is DriveItem d) StartScan(d.RootPath);
        });
        ScanPathCommand = new RelayCommand(p =>
        {
            if (p is QuickFolder q) StartScan(q.Path);
            else if (p is string s) StartScan(s);
        });
        BrowseCommand = new RelayCommand(Browse);
        CancelCommand = new RelayCommand(() => _cts?.Cancel());
        NewScanCommand = new RelayCommand(() =>
        {
            LoadDrives();
            State = AnalyzerState.Idle;
        });
        RescanCommand = new RelayCommand(() =>
        {
            if (_scanPath != null) StartScan(_scanPath);
        });
        UpCommand = new RelayCommand(() =>
        {
            if (Current?.Parent != null) NavigateTo(Current.Parent, Current);
        }, () => Current?.Parent != null);
        CrumbCommand = new RelayCommand(p =>
        {
            if (p is Crumb c && c.Node != Current) NavigateTo(c.Node, null);
        });
        OpenItemCommand = new RelayCommand(p => OpenItem(p as NodeItem ?? SelectedChild));
        ShowInExplorerCommand = new RelayCommand(p =>
        {
            if (p is NodeItem { CanAct: true } n) ShellHelper.ShowInExplorer(n.FullPath);
        });
        CopyPathCommand = new RelayCommand(p =>
        {
            if (p is NodeItem { CanAct: true } n)
            {
                ShellHelper.CopyText(n.FullPath);
                _main.ShowToast("Путь скопирован", "info");
            }
        });
        DeleteCommand = new RelayCommand(async p =>
        {
            if ((p as NodeItem ?? SelectedChild) is { CanAct: true } n) await DeleteAsync(n);
        });
        OpenLargeFileFolderCommand = new RelayCommand(p =>
        {
            if (p is LargeFile f) ShellHelper.ShowInExplorer(f.FullPath);
        });
        DeleteLargeFileCommand = new RelayCommand(async p =>
        {
            if (p is LargeFile f)
                await DeleteAsync(new NodeItem { Name = f.Name, Size = f.Size, Parent = f.Parent, FileName = f.Name, Category = f.Category });
        });

        CompressCommand = new RelayCommand(async p =>
        {
            if ((p as NodeItem ?? SelectedChild) is { IsDir: true, CanAct: true } n) await CompressAsync(n);
        });
        MoveCommand = new RelayCommand(async p =>
        {
            if ((p as NodeItem ?? SelectedChild) is { IsDir: true, CanAct: true } n) await MoveAsync(n);
        });
        ExcludeCommand = new RelayCommand(p =>
        {
            if ((p as NodeItem ?? SelectedChild) is not { CanAct: true } n) return;
            var settings = AppSettings.Current;
            if (!settings.Exclusions.Contains(n.FullPath, StringComparer.OrdinalIgnoreCase))
            {
                settings.Exclusions.Add(n.FullPath);
                settings.Save();
            }
            _main.ShowToast($"«{n.Name}» добавлена в исключения — «Простор» не будет её трогать", "info");
        });
        OpenChangeCommand = new RelayCommand(p =>
        {
            if (p is not FolderChange c) return;
            var node = ScanStats.FindNode(Root, c.FullPath);
            if (node != null)
            {
                Tab = "structure";
                NavigateTo(node, null);
            }
            else ShellHelper.OpenFolder(c.FullPath);
        });
        ToggleHighlightCommand = new RelayCommand(p =>
        {
            if (p is not LegendItem item) return;
            int cat = (int)item.Info.Category;
            HighlightCategory = HighlightCategory == cat ? -1 : cat;
            foreach (var l in Legend) l.IsDimmed = HighlightCategory >= 0 && (int)l.Info.Category != HighlightCategory;
        });
        FindDuplicatesCommand = new RelayCommand(async () => await FindDuplicatesAsync(), () => Root != null && !IsSearchingDups);
        CancelDuplicatesCommand = new RelayCommand(() => _dupCts?.Cancel());
        KeepNewestCommand = new RelayCommand(() => MarkDuplicates(keepNewest: true));
        KeepOldestCommand = new RelayCommand(() => MarkDuplicates(keepNewest: false));
        ClearDupMarksCommand = new RelayCommand(() =>
        {
            foreach (var g in DupGroups)
                foreach (var f in g.Files) f.IsChecked = false;
        });
        DeleteDuplicatesCommand = new RelayCommand(async () => await DeleteDuplicatesAsync(), () => DupSelectedCount > 0 && !IsSearchingDups);
        ShowDuplicateCommand = new RelayCommand(p =>
        {
            if (p is DuplicateFile f) ShellHelper.ShowInExplorer(f.Path);
        });

        Legend = FileCategories.All.Select(c => new LegendItem { Info = c }).ToList();
        LoadDrives();
        LoadQuickFolders();
    }

    // ------------------------------------------------------------------ state

    private AnalyzerState _state = AnalyzerState.Idle;
    public AnalyzerState State
    {
        get => _state;
        set
        {
            if (Set(ref _state, value)) OnPropertyChanged(nameof(IsScanning));
        }
    }

    public bool IsScanning => _state == AnalyzerState.Scanning;

    private string _tab = "structure";
    public string Tab { get => _tab; set => Set(ref _tab, value); }

    private string _scanTitle;
    public string ScanTitle { get => _scanTitle; set => Set(ref _scanTitle, value); }

    private string _progressFiles, _progressSize, _progressTime, _progressPath;
    public string ProgressFiles { get => _progressFiles; set => Set(ref _progressFiles, value); }
    public string ProgressSize { get => _progressSize; set => Set(ref _progressSize, value); }
    public string ProgressTime { get => _progressTime; set => Set(ref _progressTime, value); }
    public string ProgressPath { get => _progressPath; set => Set(ref _progressPath, value); }

    private double _progress;
    public double Progress { get => _progress; set => Set(ref _progress, value); }

    private bool _hasProgress;
    public bool HasProgress { get => _hasProgress; set => Set(ref _hasProgress, value); }

    private DirNode _root;
    public DirNode Root { get => _root; private set => Set(ref _root, value); }

    private DirNode _current;
    public DirNode Current { get => _current; private set => Set(ref _current, value); }

    private ObservableCollection<NodeItem> _children = new();
    public ObservableCollection<NodeItem> Children { get => _children; private set => Set(ref _children, value); }

    private NodeItem _selectedChild;
    public NodeItem SelectedChild
    {
        get => _selectedChild;
        set
        {
            if (Set(ref _selectedChild, value)) SelectedKey = value?.TreemapKey;
        }
    }

    private object _selectedKey;
    public object SelectedKey { get => _selectedKey; set => Set(ref _selectedKey, value); }

    private int _treemapVersion;
    public int TreemapVersion { get => _treemapVersion; set => Set(ref _treemapVersion, value); }

    private string _hoverText;
    public string HoverText { get => _hoverText; set => Set(ref _hoverText, value); }

    private string _currentSummary;
    public string CurrentSummary { get => _currentSummary; set => Set(ref _currentSummary, value); }

    private string _resultSummary;
    public string ResultSummary { get => _resultSummary; set => Set(ref _resultSummary, value); }

    private List<LargeFile> _largeFiles = new();
    public List<LargeFile> LargeFiles { get => _largeFiles; set => Set(ref _largeFiles, value); }

    private List<CategoryStat> _categories = new();
    public List<CategoryStat> Categories { get => _categories; set => Set(ref _categories, value); }

    private List<ExtStat> _extensions = new();
    public List<ExtStat> Extensions { get => _extensions; set => Set(ref _extensions, value); }

    private bool _statsReady;
    public bool StatsReady { get => _statsReady; set => Set(ref _statsReady, value); }

    public List<LegendItem> Legend { get; }

    private int _highlightCategory = -1;
    public int HighlightCategory { get => _highlightCategory; set => Set(ref _highlightCategory, value); }

    // ---- large / old files
    private List<LargeFile> _oldFiles = new(), _oldDownloads = new();
    private long _oldFilesSize, _oldDownloadsSize;

    private string _filesFilter = "all";
    public string FilesFilter
    {
        get => _filesFilter;
        set
        {
            if (!Set(ref _filesFilter, value)) return;
            OnPropertyChanged(nameof(FilesList));
            OnPropertyChanged(nameof(FilesNote));
        }
    }

    public List<LargeFile> FilesList => _filesFilter switch
    {
        "old" => _oldFiles,
        "downloads" => _oldDownloads,
        _ => LargeFiles
    };

    public string FilesNote => _filesFilter switch
    {
        "old" => _oldFiles.Count == 0
            ? "Файлов крупнее 10 МБ, которые не открывали больше года, не нашлось."
            : $"Файлы крупнее 10 МБ, которые не открывали и не меняли больше года: всего {Fmt.Size(_oldFilesSize)}. Возможно, их пора удалить или перенести в архив.",
        "downloads" => _oldDownloads.Count == 0
            ? "В «Загрузках» нет крупных файлов старше трёх месяцев (или эта папка не входит в сканирование)."
            : $"Крупные файлы, которые лежат в «Загрузках» дольше трёх месяцев: {Fmt.Size(_oldDownloadsSize)}. Обычно это установщики и архивы, которые уже не нужны.",
        _ => null
    };

    public string OldFilesCount => _oldFiles.Count > 0 ? Fmt.Size(_oldFilesSize) : null;
    public string OldDownloadsCount => _oldDownloads.Count > 0 ? Fmt.Size(_oldDownloadsSize) : null;

    // ---- changes since the previous scan
    private ScanComparison _comparison;
    public ScanComparison Comparison
    {
        get => _comparison;
        private set
        {
            Set(ref _comparison, value);
            OnPropertyChanged(nameof(HasComparison));
            OnPropertyChanged(nameof(ComparisonTitle));
            OnPropertyChanged(nameof(ComparisonNote));
        }
    }

    public bool HasComparison => _comparison != null;
    public string ComparisonTitle => _comparison == null
        ? "Это первое сканирование этой папки"
        : $"С {Fmt.Date(_comparison.PreviousTime)} ({Fmt.Ago(_comparison.PreviousTime)}): " +
          (_comparison.Delta >= 0 ? "+" : "−") + Fmt.Size(Math.Abs(_comparison.Delta));
    public string ComparisonNote => _comparison == null
        ? "«Простор» запомнил размеры папок. Просканируйте её снова через несколько дней — здесь появится, что выросло и что уменьшилось."
        : _comparison.Changes.Count == 0
            ? "Заметных изменений нет — ни одна папка не изменилась больше чем на 50 МБ."
            : "Папки, размер которых заметно изменился. Нажмите на строку, чтобы открыть папку на карте.";

    // ---- scan engine
    private bool _usedMft;
    public bool UsedMft { get => _usedMft; private set => Set(ref _usedMft, value); }

    // ------------------------------------------------------------------ setup

    public void LoadDrives()
    {
        Drives.Clear();
        foreach (var d in DriveService.GetDrives()) Drives.Add(d);
    }

    private void LoadQuickFolders()
    {
        string profile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        var list = new List<QuickFolder>
        {
            new() { Title = "Загрузки", Path = ShellHelper.DownloadsFolder, Glyph = "" },
            new() { Title = "Документы", Path = Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), Glyph = "" },
            new() { Title = "Рабочий стол", Path = Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory), Glyph = "" },
            new() { Title = "Видео", Path = Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), Glyph = "" },
            new() { Title = "Изображения", Path = Environment.GetFolderPath(Environment.SpecialFolder.MyPictures), Glyph = "" },
            new() { Title = "AppData", Path = Path.Combine(profile, "AppData"), Glyph = "" },
            new() { Title = "Профиль пользователя", Path = profile, Glyph = "" },
        };
        foreach (var q in list.Where(q => !string.IsNullOrEmpty(q.Path) && Directory.Exists(q.Path)))
            QuickFolders.Add(q);
    }

    private void Browse()
    {
        var dlg = new OpenFolderDialog { Title = "Выберите папку для анализа" };
        if (dlg.ShowDialog() == true) StartScan(dlg.FolderName);
    }

    // ------------------------------------------------------------------ scanning

    /// <param name="tab">Tab to open when the scan is done ("structure" by default).</param>
    /// <param name="filesFilter">Filter of the files tab ("all", "old", "downloads").</param>
    public async void StartScan(string path, string tab = null, string filesFilter = null)
    {
        if (string.IsNullOrWhiteSpace(path) || !Directory.Exists(path))
        {
            _main.ShowToast("Папка не найдена", "warning");
            return;
        }
        _main.Navigate("analyzer");
        _cts?.Cancel();
        var cts = _cts = new CancellationTokenSource();
        _scanPath = path;

        bool isDrive = path.Length <= 3;
        bool mft = isDrive && MftScanner.CanUse(path);
        object scanner = _scanner = mft ? new MftScanner() : new DiskScanner();
        DriveInfo drive = null;
        try
        {
            if (isDrive) drive = new DriveInfo(path);
        }
        catch { }
        _expectedBytes = drive is { IsReady: true } ? drive.TotalSize - drive.TotalFreeSpace : 0;
        HasProgress = _expectedBytes > 0;
        Progress = 0;
        ScanTitle = isDrive ? $"Сканируем диск {path.TrimEnd('\\')}" : $"Сканируем «{Path.GetFileName(path.TrimEnd('\\'))}»";
        if (mft)
        {
            ScanTitle += " — быстрый режим";
            HasProgress = true;
        }
        ProgressFiles = ProgressSize = ProgressTime = "";
        ProgressPath = path;
        State = AnalyzerState.Scanning;
        StatsReady = false;

        _watch = Stopwatch.StartNew();
        _timer?.Stop();
        _timer = new DispatcherTimer(DispatcherPriority.Background) { Interval = TimeSpan.FromMilliseconds(120) };
        _timer.Tick += (_, _) => UpdateProgress(scanner);
        _timer.Start();

        DirNode root;
        try
        {
            root = await Task.Run(() =>
            {
                if (scanner is MftScanner m)
                {
                    try
                    {
                        return m.Scan(path, cts.Token);
                    }
                    catch (Exception ex) when (ex is not OperationCanceledException)
                    {
                        App.Log(ex); // fall back to walking the folders
                        var fallback = new DiskScanner();
                        _scanner = fallback;
                        return fallback.Scan(path, cts.Token);
                    }
                }
                return ((DiskScanner)scanner).Scan(path, cts.Token);
            }, cts.Token);
            mft = _scanner is MftScanner;
        }
        catch (OperationCanceledException)
        {
            if (_cts == cts)
            {
                _timer.Stop();
                State = Root != null ? AnalyzerState.Done : AnalyzerState.Idle;
                _main.ShowToast("Сканирование остановлено", "info");
            }
            return;
        }
        catch (Exception ex)
        {
            _timer.Stop();
            State = AnalyzerState.Idle;
            await _main.ShowError("Не удалось просканировать", ex.Message);
            return;
        }
        if (_cts != cts) return;
        _timer.Stop();
        _watch.Stop();

        if (isDrive && _expectedBytes > 0)
        {
            long unknown = _expectedBytes - root.Size;
            if (unknown > Math.Max(64L << 20, _expectedBytes / 200))
            {
                var node = new DirNode { Name = "Системные и недоступные данные", Parent = root, Size = unknown, Kind = NodeKind.Unknown };
                root.Dirs.Add(node);
                root.Dirs.Sort(static (a, b) => b.Size.CompareTo(a.Size));
                root.Size += unknown;
            }
        }

        UsedMft = mft;
        ResultSummary = $"{Fmt.Files(root.FileCount)} · {Fmt.Folders(root.DirCount)} · за {Fmt.Duration(_watch.Elapsed)}" +
                        (mft ? " · быстрый режим (MFT)" : "");
        Root = root;
        Tab = tab ?? "structure";
        FilesFilter = filesFilter ?? "all";
        NavigateTo(root, null);
        State = AnalyzerState.Done;
        HighlightCategory = -1;
        foreach (var l in Legend) l.IsDimmed = false;
        DupGroups.Clear();
        DupsSearched = false;
        RaiseDupSelection();
        _ = CompareWithPreviousAsync(root, path);
        await ComputeStatsAsync();
    }

    private async Task CompareWithPreviousAsync(DirNode root, string path)
    {
        try
        {
            var comparison = await Task.Run(() =>
            {
                var current = ScanHistory.Capture(root, path);
                var previous = ScanHistory.LoadPrevious(path, TimeSpan.FromMinutes(30));
                ScanHistory.Save(current);
                return previous == null ? null : ScanHistory.Compare(previous, current);
            });
            if (root != Root) return;
            Comparison = comparison;
        }
        catch (Exception ex)
        {
            App.Log(ex);
        }
    }

    private void UpdateProgress(object scanner)
    {
        ProgressTime = Fmt.Duration(_watch.Elapsed);
        switch (_scanner ?? scanner)
        {
            case MftScanner m:
                long total = Math.Max(1, m.RecordsTotal);
                ProgressFiles = m.RecordsTotal == 0
                    ? "Читаем таблицу файлов NTFS…"
                    : $"Таблица файлов NTFS: {Fmt.Number(m.RecordsRead)} из {Fmt.Number(m.RecordsTotal)} записей";
                ProgressSize = Fmt.Size(m.BytesScanned);
                ProgressPath = string.IsNullOrEmpty(m.CurrentPath) ? _scanPath : m.CurrentPath;
                Progress = Math.Min(0.99, 0.9 * m.RecordsRead / total + (_expectedBytes > 0 ? 0.1 * m.BytesScanned / _expectedBytes : 0));
                break;
            case DiskScanner s:
                ProgressFiles = $"{Fmt.Number(s.FilesScanned)} файлов · {Fmt.Number(s.DirsScanned)} папок";
                ProgressSize = Fmt.Size(s.BytesScanned);
                ProgressPath = s.CurrentPath;
                if (_expectedBytes > 0) Progress = Math.Min(0.99, (double)s.BytesScanned / _expectedBytes);
                break;
        }
    }

    private async Task ComputeStatsAsync()
    {
        var root = Root;
        if (root == null) return;
        var stats = await Task.Run(() => ScanStats.Compute(root));
        if (root != Root) return;
        LargeFiles = stats.LargestFiles;
        _oldFiles = stats.OldFiles;
        _oldDownloads = stats.OldDownloads;
        _oldFilesSize = stats.OldFilesSize;
        _oldDownloadsSize = stats.OldDownloadsSize;
        OnPropertyChanged(nameof(FilesList));
        OnPropertyChanged(nameof(FilesNote));
        OnPropertyChanged(nameof(OldFilesCount));
        OnPropertyChanged(nameof(OldDownloadsCount));
        Categories = stats.Categories;
        Extensions = stats.Extensions;
        StatsReady = true;
    }

    // ------------------------------------------------------------------ navigation

    public void NavigateTo(DirNode node, DirNode selectChild)
    {
        if (node == null) return;
        Current = node;
        RebuildChildren();
        Breadcrumbs.Clear();
        var chain = new List<DirNode>();
        for (var p = node; p != null; p = p.Parent) chain.Insert(0, p);
        for (int i = 0; i < chain.Count; i++)
            Breadcrumbs.Add(new Crumb { Name = i == 0 ? chain[i].Name.TrimEnd('\\') : chain[i].Name, Node = chain[i], IsFirst = i == 0 });
        SelectedChild = selectChild == null ? null : Children.FirstOrDefault(c => c.Dir == selectChild);
        HoverText = null;
        CommandManager.InvalidateRequerySuggested();
    }

    private void RebuildChildren()
    {
        var cur = Current;
        if (cur == null)
        {
            Children = new ObservableCollection<NodeItem>();
            return;
        }
        double total = Math.Max(1, cur.Size);
        var list = new List<NodeItem>(cur.Dirs.Count + Math.Min(cur.Files.Length, MaxFileRows) + 1);
        foreach (var d in cur.Dirs)
            list.Add(new NodeItem { Name = d.Name, Size = d.Size, Fraction = d.Size / total, Dir = d, LastWrite = d.LastWrite });
        int shown = Math.Min(cur.Files.Length, MaxFileRows);
        for (int i = 0; i < shown; i++)
        {
            var f = cur.Files[i];
            list.Add(new NodeItem
            {
                Name = f.Name, Size = f.Size, Fraction = f.Size / total, Parent = cur, FileName = f.Name, LastWrite = f.LastWrite,
                Category = f.Category
            });
        }
        if (cur.Files.Length > shown)
        {
            long rest = 0;
            for (int i = shown; i < cur.Files.Length; i++) rest += cur.Files[i].Size;
            list.Add(new NodeItem
            {
                Name = $"Ещё {Fmt.Files(cur.Files.Length - shown)}", Size = rest, Fraction = rest / total, Parent = cur, IsRest = true,
                RestCount = cur.Files.Length - shown
            });
        }
        list.Sort(static (a, b) => b.Size.CompareTo(a.Size));
        Children = new ObservableCollection<NodeItem>(list);
        CurrentSummary = $"{Fmt.Size(cur.Size)} · {Fmt.Files(cur.FileCount)} · {Fmt.Folders(cur.DirCount)}";
        TreemapVersion++;
    }

    private void OpenItem(NodeItem item)
    {
        if (item == null || item.IsRest) return;
        if (item.IsDir)
        {
            if (!item.Dir.IsVirtual && (item.Dir.Dirs.Count > 0 || item.Dir.Files.Length > 0)) NavigateTo(item.Dir, null);
        }
        else
        {
            ShellHelper.ShowInExplorer(item.FullPath);
        }
    }

    // ------------------------------------------------------------------ treemap interaction

    private DirNode DirectChildOf(DirNode ancestor, DirNode node)
    {
        for (var p = node; p != null; p = p.Parent)
            if (p.Parent == ancestor) return p;
        return null;
    }

    public void SelectByHit(TreemapHit hit)
    {
        if (hit == null || Current == null) return;
        if (hit.IsFile && hit.FileParent == Current)
        {
            SelectedChild = Children.FirstOrDefault(c => !c.IsDir && !c.IsRest &&
                                                         string.Equals(c.FileName, hit.FileName, StringComparison.OrdinalIgnoreCase));
            return;
        }
        var owner = hit.IsFile || hit.IsRest ? hit.FileParent : hit.Dir;
        var direct = owner == Current ? null : DirectChildOf(Current, owner);
        if (direct != null) SelectedChild = Children.FirstOrDefault(c => c.Dir == direct);
        SelectedKey = hit.IsFile ? new FileRef(hit.FileParent, hit.FileName) : hit.IsRest ? null : hit.Dir;
    }

    public void DrillByHit(TreemapHit hit)
    {
        if (hit == null) return;
        if (hit.IsFile)
        {
            if (hit.FileParent != Current) NavigateTo(hit.FileParent, null);
            else ShellHelper.ShowInExplorer(hit.FullPath);
            return;
        }
        var target = hit.IsRest ? hit.FileParent : hit.Dir;
        if (target == null || target.IsVirtual || target == Current) return;
        NavigateTo(target, null);
    }

    public NodeItem ItemFromHit(TreemapHit hit)
    {
        if (hit == null || hit.IsRest) return null;
        if (hit.IsFile)
            return new NodeItem { Name = hit.FileName, Size = hit.Size, Parent = hit.FileParent, FileName = hit.FileName, Category = hit.Category };
        return new NodeItem { Name = hit.Dir.Name, Size = hit.Size, Dir = hit.Dir };
    }

    public void SetHover(TreemapHit hit)
    {
        HoverText = hit == null ? null : $"{hit.FullPath}   ·   {Fmt.Size(hit.Size)}";
    }

    // ------------------------------------------------------------------ duplicates

    public ObservableCollection<DuplicateGroup> DupGroups { get; } = new();
    private CancellationTokenSource _dupCts;

    private bool _isSearchingDups;
    public bool IsSearchingDups { get => _isSearchingDups; private set => Set(ref _isSearchingDups, value); }

    private bool _dupsSearched;
    public bool DupsSearched { get => _dupsSearched; private set => Set(ref _dupsSearched, value); }

    private double _dupProgress;
    public double DupProgress { get => _dupProgress; private set => Set(ref _dupProgress, value); }

    private string _dupStatus;
    public string DupStatus { get => _dupStatus; private set => Set(ref _dupStatus, value); }

    private string _dupSummary;
    public string DupSummary { get => _dupSummary; private set => Set(ref _dupSummary, value); }

    private string _dupMinMb = "1";
    /// <summary>Smallest file size taken into account: "1", "10" or "100" MB.</summary>
    public string DupMinMb { get => _dupMinMb; set => Set(ref _dupMinMb, value); }

    private bool _dupPersonalOnly = true;
    /// <summary>Skip Windows, Program Files, AppData and program folders: copies there are usually needed.</summary>
    public bool DupPersonalOnly { get => _dupPersonalOnly; set => Set(ref _dupPersonalOnly, value); }

    public int DupSelectedCount => DupGroups.Sum(g => g.Files.Count(f => f.IsChecked));
    public long DupSelectedSize => DupGroups.Sum(g => g.Files.Count(f => f.IsChecked) * g.Size);
    public string DupSelectedText => DupSelectedCount == 0
        ? "Отметьте лишние копии или нажмите «Оставить самые новые»"
        : $"Отмечено {Fmt.Files(DupSelectedCount)} · {Fmt.Size(DupSelectedSize)}";

    private void RaiseDupSelection()
    {
        OnPropertyChanged(nameof(DupSelectedCount));
        OnPropertyChanged(nameof(DupSelectedSize));
        OnPropertyChanged(nameof(DupSelectedText));
        CommandManager.InvalidateRequerySuggested();
    }

    private async Task FindDuplicatesAsync()
    {
        var root = Root;
        if (root == null) return;
        _dupCts?.Cancel();
        var cts = _dupCts = new CancellationTokenSource();
        var finder = new DuplicateFinder();
        long minSize = long.Parse(_dupMinMb) << 20;
        var programFolders = _main.Programs.Apps
            .SelectMany(a => a.ExtraLocations.Append(a.InstallLocation))
            .Where(p => !string.IsNullOrEmpty(p))
            .ToList();
        Func<string, bool> skip = _dupPersonalOnly
            ? DuplicateFinder.DefaultSkip(programFolders)
            : p => AppSettings.Current.IsExcluded(p) || Path.GetFileName(p) is "$Recycle.Bin" or "System Volume Information";

        DupGroups.Clear();
        RaiseDupSelection();
        IsSearchingDups = true;
        DupProgress = 0;
        DupStatus = "Сравниваем размеры файлов…";
        var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(200) };
        timer.Tick += (_, _) =>
        {
            if (finder.GroupsTotal > 0)
            {
                DupProgress = (double)finder.GroupsDone / finder.GroupsTotal;
                DupStatus = $"Сравниваем содержимое: {Fmt.Number(finder.GroupsDone)} из {Fmt.Number(finder.GroupsTotal)} групп файлов одинакового размера · прочитано {Fmt.Size(finder.HashedBytes)}";
            }
        };
        timer.Start();
        try
        {
            var groups = await Task.Run(() => finder.Find(root, minSize, skip, cts.Token), cts.Token);
            if (root != Root) return;
            foreach (var g in groups.Take(500))
            {
                g.CheckedChanged += RaiseDupSelection;
                DupGroups.Add(g);
            }
            long wasted = groups.Sum(g => g.Wasted);
            DupSummary = groups.Count == 0
                ? "Одинаковых файлов не найдено."
                : $"Найдено {Fmt.Count(groups.Count, "группа", "группы", "групп")} одинаковых файлов — лишние копии занимают {Fmt.Size(wasted)}" +
                  (groups.Count > 500 ? ". Показаны 500 самых крупных групп." : ".");
            DupsSearched = true;
        }
        catch (OperationCanceledException)
        {
            DupSummary = null;
        }
        finally
        {
            timer.Stop();
            IsSearchingDups = false;
            RaiseDupSelection();
        }
    }

    /// <summary>Marks every copy except one (the newest or the oldest) in each group.</summary>
    private void MarkDuplicates(bool keepNewest)
    {
        foreach (var g in DupGroups)
        {
            var keep = keepNewest ? g.Files.MaxBy(f => f.LastWrite) : g.Files.MinBy(f => f.LastWrite);
            foreach (var f in g.Files) f.IsChecked = f != keep && !AppSettings.Current.IsExcluded(f.Path);
        }
    }

    private async Task DeleteDuplicatesAsync()
    {
        var files = DupGroups.SelectMany(g => g.Files.Where(f => f.IsChecked)).ToList();
        if (files.Count == 0) return;
        // never remove every copy of a file
        var whole = DupGroups.Where(g => g.Files.All(f => f.IsChecked)).ToList();
        if (whole.Count > 0)
        {
            await _main.ShowError("Отмечены все копии", $"В {Fmt.Count(whole.Count, "группе", "группах", "группах")} отмечены все копии файла (например, «{whole[0].Name}»). Оставьте хотя бы одну.");
            return;
        }
        long size = DupGroups.Sum(g => g.Files.Count(f => f.IsChecked) * g.Size);
        var ok = await _main.ShowDialog(new DialogViewModel
        {
            Title = $"Переместить {Fmt.Files(files.Count)} в корзину?",
            Message = $"Лишние копии ({Fmt.Size(size)}) будут перемещены в корзину. В каждой группе останется хотя бы один файл. Вернуть их можно из «Журнала».",
            Glyph = "\uE74D", Kind = "danger", IsDanger = true, PrimaryText = "В корзину", SecondaryText = "Отмена"
        });
        if (!ok) return;

        var busy = new DialogViewModel { Title = "Перемещаем в корзину…", Glyph = "\uE74D", IsBusy = true, PrimaryText = null, ShowProgress = true, IsIndeterminate = false };
        _ = _main.ShowDialog(busy);
        var moved = new List<string>();
        long freed = 0;
        for (int i = 0; i < files.Count; i++)
        {
            var f = files[i];
            busy.ProgressText = f.Path;
            busy.Progress = (double)i / files.Count;
            if (await Task.Run(() => ShellHelper.MoveToRecycleBin(f.Path)))
            {
                moved.Add(f.Path);
                freed += f.Group.Size;
                var parent = ScanStats.FindNode(Root, f.Folder);
                parent?.RemoveFile(f.Name);
            }
        }
        busy.Close(true);
        foreach (var g in DupGroups.ToList())
        {
            g.Files.RemoveAll(f => moved.Contains(f.Path));
            if (g.Files.Count < 2) DupGroups.Remove(g);
        }
        // re-create the groups so the lists refresh
        var rest = DupGroups.ToList();
        DupGroups.Clear();
        foreach (var g in rest) DupGroups.Add(g);
        RaiseDupSelection();
        if (moved.Count > 0)
            ActionJournal.Add("duplicates", $"Удалены дубликаты: {Fmt.Files(moved.Count)}", "Перемещены в корзину", freed, UndoKind.RecycleRestore, moved);
        _main.ShowToast(moved.Count == files.Count ? $"Перемещено в корзину: {Fmt.Files(moved.Count)} · {Fmt.Size(freed)}"
            : $"Перемещено {moved.Count} из {files.Count} — часть файлов занята", moved.Count == files.Count ? "success" : "warning");
        NavigateTo(Current ?? Root, null);
        await ComputeStatsAsync();
    }

    // ------------------------------------------------------------------ compression & moving

    private async Task CompressAsync(NodeItem item)
    {
        string path = item.FullPath;
        if (!Compression.IsSupported(path))
        {
            await _main.ShowError("Сжатие недоступно", "Сжимать можно только папки на дисках NTFS.");
            return;
        }
        var estimate = new DialogViewModel
        {
            Title = $"Оцениваем «{item.Name}»…", Glyph = "\uE8D1", IsBusy = true, PrimaryText = null, ShowProgress = true
        };
        _ = _main.ShowDialog(estimate);
        double saving = await Task.Run(() => Compression.EstimateSaving(path));
        estimate.Close(true);
        long expected = (long)(item.Size * saving);

        var ask = new DialogViewModel
        {
            Title = $"Сжать «{item.Name}»?",
            Message = (saving < 0.05
                          ? "Файлы в этой папке уже сжаты (видео, архивы, картинки) — освободится совсем немного места."
                          : $"Примерно освободится {Fmt.Size(expected)} из {Fmt.Size(item.Size)} (оценка по выборке файлов).") +
                      "\n\nWindows сожмёт файлы прозрачно: программы и игры продолжат работать как раньше, просто займут меньше места. " +
                      "Скорость чтения обычно не меняется. Обновления программ могут записать часть файлов заново несжатыми.",
            Glyph = "\uE8D1",
            PrimaryText = "Сжать",
            SecondaryText = "Отмена",
            Footnote = ShellHelper.IsAdmin ? null : "Папки в Program Files можно сжать только в режиме администратора."
        };
        if (!await _main.ShowDialog(ask)) return;

        var busy = new DialogViewModel
        {
            Title = $"Сжимаем «{item.Name}»…", Message = "Можно продолжать пользоваться компьютером.", Glyph = "\uE8D1",
            IsBusy = true, PrimaryText = null, ShowProgress = true, IsIndeterminate = false, SecondaryText = "Остановить"
        };
        _ = _main.ShowDialog(busy);
        using var cts = new CancellationTokenSource();
        _ = busy.Result.ContinueWith(_ =>
        {
            if (busy.Choice == "secondary") cts.Cancel();
        }, TaskScheduler.Default);
        try
        {
            long before = await Task.Run(() => Compression.PhysicalSize(path));
            await Compression.CompressAsync(path, new Progress<(int done, int total)>(p =>
            {
                busy.Progress = p.total > 0 ? (double)p.done / p.total : 0;
                busy.ProgressText = $"{Fmt.Number(p.done)} из {Fmt.Files(p.total)}";
            }), cts.Token);
            long after = await Task.Run(() => Compression.PhysicalSize(path));
            busy.Close(true);
            long freed = Math.Max(0, before - after);
            ActionJournal.Add("compress", $"Сжата папка «{item.Name}»", path, freed, UndoKind.Decompress, new[] { path });
            await _main.ShowDialog(new DialogViewModel
            {
                Title = freed > 0 ? $"Освобождено {Fmt.Size(freed)}" : "Готово",
                Message = $"На диске папка теперь занимает {Fmt.Size(after)} вместо {Fmt.Size(before)}. Сжатие можно отменить в «Журнале».",
                Glyph = "\uE73E", Kind = "success", PrimaryText = "Отлично"
            });
            _main.Dashboard.Refresh();
        }
        catch (OperationCanceledException)
        {
            busy.Close(false);
            ActionJournal.Add("compress", $"Частично сжата папка «{item.Name}»", path, 0, UndoKind.Decompress, new[] { path });
            _main.ShowToast("Сжатие остановлено — уже сжатые файлы остались сжатыми", "info");
        }
        catch (Exception ex)
        {
            busy.Close(false);
            await _main.ShowError("Не удалось сжать папку", ex.Message);
        }
    }

    private async Task MoveAsync(NodeItem item)
    {
        string path = item.FullPath;
        string reason = FolderMover.CheckCanMove(path);
        if (reason != null)
        {
            await _main.ShowError("Эту папку нельзя перенести", reason);
            return;
        }
        var drives = FolderMover.TargetDrives(path, item.Size);
        if (drives.Count == 0)
        {
            await _main.ShowError("Некуда переносить", $"Нет другого диска NTFS, на котором поместится {Fmt.Size(item.Size)}.");
            return;
        }
        var target = drives.OrderByDescending(d => d.AvailableFreeSpace).First();
        string parent = Path.Combine(target.RootDirectory.FullName, "Prostor", "Перенесено");
        var ask = new DialogViewModel
        {
            Title = $"Перенести «{item.Name}» на диск {target.Name.TrimEnd('\\')}?",
            Message = $"Папка ({Fmt.Size(item.Size)}) будет скопирована в {parent}, а на старом месте останется ссылка — " +
                      "программы продолжат находить файлы по прежнему пути. Освободится место на диске " +
                      $"{Path.GetPathRoot(path)?.TrimEnd('\\')}.\n\nПеред переносом закройте программы, которые используют эту папку. Вернуть всё обратно можно из «Журнала».",
            Glyph = "\uE8DE",
            PrimaryText = "Перенести",
            SecondaryText = "Отмена",
            TertiaryText = "Выбрать папку…"
        };
        await _main.ShowDialog(ask);
        if (ask.Choice == "tertiary")
        {
            var dlg = new OpenFolderDialog { Title = "Куда перенести папку", InitialDirectory = target.RootDirectory.FullName };
            if (dlg.ShowDialog() != true) return;
            if (string.Equals(Path.GetPathRoot(dlg.FolderName), Path.GetPathRoot(path), StringComparison.OrdinalIgnoreCase))
            {
                await _main.ShowError("Тот же диск", "Выберите папку на другом диске — перенос внутри одного диска не освободит место.");
                return;
            }
            parent = dlg.FolderName;
        }
        else if (ask.Choice != "primary") return;

        var busy = new DialogViewModel
        {
            Title = $"Переносим «{item.Name}»…", Glyph = "\uE8DE", IsBusy = true, PrimaryText = null, ShowProgress = true, IsIndeterminate = false,
            SecondaryText = "Отменить"
        };
        _ = _main.ShowDialog(busy);
        using var cts = new CancellationTokenSource();
        _ = busy.Result.ContinueWith(_ =>
        {
            if (busy.Choice == "secondary") cts.Cancel();
        }, TaskScheduler.Default);
        try
        {
            Directory.CreateDirectory(parent);
            string newPath = await FolderMover.MoveAsync(path, parent, new Progress<double>(p =>
            {
                busy.Progress = p;
                busy.ProgressText = $"Скопировано {Fmt.Percent(p)} из {Fmt.Size(item.Size)}";
            }), cts.Token);
            busy.Close(true);
            ActionJournal.Add("move", $"«{item.Name}» перенесена на диск {Path.GetPathRoot(newPath)?.TrimEnd('\\')}", $"{path} → {newPath}",
                item.Size, UndoKind.MoveBack, new[] { path, newPath });
            if (item.Dir?.Parent is { } p) p.RemoveDir(item.Dir);
            NavigateTo(Current ?? Root, null);
            _main.ShowToast($"Готово: освобождено {Fmt.Size(item.Size)} на диске {Path.GetPathRoot(path)?.TrimEnd('\\')}");
            _main.Dashboard.Refresh();
        }
        catch (OperationCanceledException)
        {
            busy.Close(false);
            _main.ShowToast("Перенос отменён — папка осталась на месте", "info");
        }
        catch (Exception ex)
        {
            busy.Close(false);
            await _main.ShowError("Не удалось перенести папку", ex.Message);
        }
    }

    // ------------------------------------------------------------------ deletion

    private async Task DeleteAsync(NodeItem item)
    {
        string path = item.FullPath;
        if (AppSettings.Current.IsExcluded(path))
        {
            await _main.ShowError("Путь в исключениях", "Он добавлен в исключения в настройках — «Простор» его не удаляет.");
            return;
        }
        var ok = await _main.ShowDialog(new DialogViewModel
        {
            Title = "Переместить в корзину?",
            Message = $"{(item.IsDir ? "Папка" : "Файл")} «{item.Name}» ({Fmt.Size(item.Size)}) будет перемещён{(item.IsDir ? "а" : "")} в корзину.\n" +
                      "Место освободится после очистки корзины.\n\n" + path,
            Glyph = "",
            Kind = "danger",
            IsDanger = true,
            PrimaryText = "В корзину",
            SecondaryText = "Отмена"
        });
        if (!ok) return;

        var busy = new DialogViewModel
        {
            Title = "Перемещаем в корзину…", Message = path, Glyph = "", IsBusy = true, PrimaryText = null,
            ShowProgress = true, IsIndeterminate = true
        };
        _ = _main.ShowDialog(busy);
        bool done = await Task.Run(() => ShellHelper.MoveToRecycleBin(path));
        busy.Close(done);
        if (!done)
        {
            await _main.ShowError("Не удалось удалить", "Возможно, файл открыт в другой программе или нет прав доступа.");
            return;
        }

        if (item.IsDir)
        {
            var parent = item.Dir.Parent;
            parent?.RemoveDir(item.Dir);
            if (Current != null && item.Dir.IsAncestorOf(Current)) Current = parent;
        }
        else
        {
            item.Parent.RemoveFile(item.FileName);
        }
        NavigateTo(Current ?? Root, null);
        ActionJournal.Add("delete", $"«{item.Name}» перемещён{(item.IsDir ? "а" : "")} в корзину", path, item.Size,
            UndoKind.RecycleRestore, new[] { path });
        _main.ShowToast($"«{item.Name}» перемещён{(item.IsDir ? "а" : "")} в корзину");
        await ComputeStatsAsync();
        _ = _main.Cleanup.ScanAsync();
        _main.Dashboard.Refresh();
    }
}
