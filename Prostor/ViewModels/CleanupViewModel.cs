using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;

namespace Prostor.ViewModels;

public sealed class CleanupViewModel : ObservableObject
{
    private readonly MainViewModel _main;
    private List<CleanupItem> _items = new();
    private CancellationTokenSource _scanCts;

    public ObservableCollection<CleanupGroup> Groups { get; } = new();

    public ICommand ScanCommand { get; }
    public ICommand CleanCommand { get; }
    public ICommand SelectAllCommand { get; }
    public ICommand OpenWindowsStorageCommand { get; }

    public event Action Changed;

    public CleanupViewModel(MainViewModel main)
    {
        _main = main;
        ScanCommand = new RelayCommand(async () => await ScanAsync(), () => !IsScanning && !IsCleaning);
        CleanCommand = new RelayCommand(async () => await CleanAsync(), () => !IsScanning && !IsCleaning && _items.Any(i => i.IsChecked && i.IsCleanable));
        SelectAllCommand = new RelayCommand(() =>
        {
            // the deep system items have side effects: "select all" leaves them as they are
            var simple = _items.Where(i => !i.IsLocked && i.Group != CleanupService.GroupDeep).ToList();
            bool all = simple.Where(i => i.Size > 0).All(i => i.IsChecked);
            foreach (var i in simple) i.IsChecked = !all;
        });
        OpenWindowsStorageCommand = new RelayCommand(() => ShellHelper.Open("ms-settings:storagesense"));
    }

    private bool _isScanning;
    public bool IsScanning { get => _isScanning; set => Set(ref _isScanning, value); }

    private bool _isCleaning;
    public bool IsCleaning { get => _isCleaning; set => Set(ref _isCleaning, value); }

    private long _selectedSize;
    public long SelectedSize { get => _selectedSize; private set => Set(ref _selectedSize, value); }

    private long _totalFound;
    public long TotalFound { get => _totalFound; private set => Set(ref _totalFound, value); }

    private string _selectedText = "Анализируем…";
    public string SelectedText { get => _selectedText; private set => Set(ref _selectedText, value); }

    private string _summaryText = "";
    public string SummaryText { get => _summaryText; private set => Set(ref _summaryText, value); }

    private long _windowsOldSize;
    public long WindowsOldSize { get => _windowsOldSize; private set => Set(ref _windowsOldSize, value); }
    public string WindowsOldText => $"Папка Windows.old занимает {Fmt.Size(WindowsOldSize)}. Её безопасно удалить через «Параметры → Система → Память → Временные файлы».";

    public bool ShowAdminHint => !ShellHelper.IsAdmin && _items.Any(i => i.RequiresAdmin);

    public int ExclusionCount => AppSettings.Current.Exclusions.Count;
    public string ExclusionText => ExclusionCount == 0
        ? null
        : $"{Fmt.Count(ExclusionCount, "папка", "папки", "папок")} в исключениях — их «Простор» не трогает";

    private static int GroupOrder(string group) => group switch
    {
        CleanupService.GroupSystem => 0,
        CleanupService.GroupBrowsers => 1,
        CleanupService.GroupApps => 2,
        CleanupService.GroupDev => 3,
        _ => 4
    };

    public async Task ScanAsync()
    {
        _scanCts?.Cancel();
        var cts = _scanCts = new CancellationTokenSource();
        IsScanning = true;
        SelectedText = "Анализируем…";

        var previous = _items.ToDictionary(i => i.Id, i => i.IsChecked);
        var saved = AppSettings.Current.CleanupChecks;
        var items = await Task.Run(CleanupService.BuildItems);
        if (cts.IsCancellationRequested) return;
        foreach (var i in items)
        {
            if (!i.IsLocked)
            {
                if (previous.TryGetValue(i.Id, out bool was)) i.IsChecked = was;
                else if (saved.TryGetValue(i.Id, out bool remembered)) i.IsChecked = remembered;
            }
            i.IsScanning = true;
            i.PropertyChanged += OnItemChanged;
        }
        foreach (var old in _items) old.PropertyChanged -= OnItemChanged;
        _items = items;

        Groups.Clear();
        foreach (var g in items.GroupBy(i => i.Group).OrderBy(g => GroupOrder(g.Key)))
        {
            Groups.Add(new CleanupGroup
            {
                Title = g.Key,
                Hint = g.Key switch
                {
                    CleanupService.GroupDev => "Кэши инструментов разработки. Они пересоздаются, но потребуют повторной загрузки.",
                    CleanupService.GroupDeep => "Системная очистка встроенными средствами Windows. Нужны права администратора; у некоторых пунктов есть последствия — они указаны.",
                    _ => null
                },
                Items = g.ToList()
            });
        }
        OnPropertyChanged(nameof(ShowAdminHint));
        OnPropertyChanged(nameof(ExclusionCount));
        OnPropertyChanged(nameof(ExclusionText));

        await Task.WhenAll(items.Select(async item =>
        {
            var (size, count) = await Task.Run(() => CleanupService.Measure(item, cts.Token));
            if (cts.IsCancellationRequested) return;
            item.SizeUnknown = size == CleanupItem.UnknownSize;
            item.Size = Math.Max(0, size);
            item.FileCount = count;
            item.IsScanning = false;
            Recalc();
        }));

        WindowsOldSize = await Task.Run(CleanupService.WindowsOldSize);
        OnPropertyChanged(nameof(WindowsOldText));

        if (cts == _scanCts)
        {
            IsScanning = false;
            Recalc();
        }
    }

    private void OnItemChanged(object sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName != nameof(CleanupItem.IsChecked)) return;
        var item = (CleanupItem)sender;
        if (!item.IsLocked)
        {
            AppSettings.Current.CleanupChecks[item.Id] = item.IsChecked;
            AppSettings.Current.Save();
        }
        Recalc();
    }

    private void Recalc()
    {
        var available = _items.Where(i => i.Size > 0).ToList();
        // items with side effects (hibernation, restore points, old drivers) are optional and not counted as junk
        TotalFound = available.Where(i => !i.IsLocked && i.Warning == null).Sum(i => i.Size);
        var selected = _items.Where(i => i.IsChecked && i.IsCleanable).ToList();
        SelectedSize = selected.Sum(i => i.Size);
        bool unknown = selected.Any(i => i.SizeUnknown);
        SelectedText = IsScanning && TotalFound == 0 ? "Анализируем…" : Fmt.Size(SelectedSize) + (unknown ? " +" : "");
        SummaryText = $"Выбрано {Fmt.Count(selected.Count, "категория", "категории", "категорий")} · всего найдено {Fmt.Size(TotalFound)}" +
                      (unknown ? " · размер части пунктов станет известен после очистки" : "");
        CommandManager.InvalidateRequerySuggested();
        _main.UpdateBadges();
        Changed?.Invoke();
    }

    private async Task CleanAsync()
    {
        var selected = _items.Where(i => i.IsChecked && i.IsCleanable).ToList();
        if (selected.Count == 0) return;

        string message = "Будут удалены временные файлы и кэши выбранных категорий. Личные файлы, пароли и настройки не затрагиваются. " +
                         "Файлы, занятые открытыми программами, будут пропущены — для полной очистки браузеров закройте их.";
        var warnings = selected.Where(i => i.Warning != null).Select(i => $"• {i.Title}: {i.Warning}").ToList();
        if (warnings.Count > 0) message += "\n\nОбратите внимание:\n" + string.Join("\n", warnings);
        if (selected.Any(i => i.IsSlow)) message += "\n\nСистемная очистка может занять несколько минут.";

        var ok = await _main.ShowDialog(new DialogViewModel
        {
            Title = SelectedSize > 0 ? $"Очистить {Fmt.Size(SelectedSize)}?" : "Запустить очистку?",
            Message = message,
            Glyph = "",
            Kind = "accent",
            PrimaryText = "Очистить",
            SecondaryText = "Отмена"
        });
        if (!ok) return;

        IsCleaning = true;
        var busy = new DialogViewModel
        {
            Title = "Очищаем…", Glyph = "", IsBusy = true, PrimaryText = null,
            ShowProgress = true, IsIndeterminate = false
        };
        _ = _main.ShowDialog(busy);
        long freed = 0, failed = 0, done = 0;
        long weight(CleanupItem i) => i.SizeUnknown ? Math.Max(1L << 30, SelectedSize / 4) : i.Size;
        long planned = Math.Max(1, selected.Sum(weight));
        var cleaned = new List<string>();
        try
        {
            for (int n = 0; n < selected.Count; n++)
            {
                var item = selected[n];
                busy.Message = item.IsSlow ? $"{item.Title} — это займёт несколько минут" : item.Title;
                busy.IsIndeterminate = item.IsSlow || item.CustomClean != null;
                busy.ProgressText = $"Категория {n + 1} из {selected.Count}  ·  освобождено {Fmt.Size(freed)}";
                var r = await Task.Run(() => CleanupService.Clean(item, CancellationToken.None));
                busy.IsIndeterminate = false;
                freed += r.FreedBytes;
                failed += r.FailedFiles;
                done += weight(item);
                if (r.FreedBytes > 0 || r.DeletedFiles > 0) cleaned.Add(item.Title);
                if (item.Id == "hibernation" && r.DeletedFiles > 0)
                    ActionJournal.Add("cleanup", "Выключена гибернация", "Удалён файл hiberfil.sys", r.FreedBytes, UndoKind.Hibernation);
                busy.Progress = Math.Min(1, (double)done / planned);
            }
            busy.ProgressText = $"Освобождено {Fmt.Size(freed)}";
            await Task.Delay(300);
        }
        finally
        {
            busy.Close(true);
            IsCleaning = false;
        }

        if (freed > 0)
            ActionJournal.Add("cleanup", "Очистка", string.Join(", ", cleaned.Take(6)) + (cleaned.Count > 6 ? $" и ещё {cleaned.Count - 6}" : ""), freed);
        _ = Task.Run(DiskHistory.Record);
        await _main.ShowDialog(new DialogViewModel
        {
            Title = $"Освобождено {Fmt.Size(freed)}",
            Message = failed > 0
                ? $"{Fmt.Files(failed)} пропущено — они используются запущенными программами. Это нормально."
                : "Всё выбранное успешно очищено.",
            Glyph = "",
            Kind = "success",
            PrimaryText = "Отлично"
        });
        await ScanAsync();
        _main.Dashboard.Refresh();
    }
}
