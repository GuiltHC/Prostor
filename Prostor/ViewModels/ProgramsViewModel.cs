using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using System.Text;
using System.Windows;
using System.Windows.Data;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;

namespace Prostor.ViewModels;

public enum AppFilter { All, Desktop, Store, Games, Large, Recent, Unused, Updates }
public enum AppSort { Size, Name, Date, LastUsed }

public sealed class ProgramsViewModel : ObservableObject
{
    private const long LargeThreshold = 1L << 30;
    private readonly MainViewModel _main;

    public ObservableCollection<InstalledApp> Apps { get; } = new();
    public ListCollectionView View { get; }

    public event Action Changed;

    public ICommand RefreshCommand { get; }
    public ICommand UninstallCommand { get; }
    public ICommand UninstallCheckedCommand { get; }
    public ICommand ForceUninstallCommand { get; }
    public ICommand ClearChecksCommand { get; }
    public ICommand OpenFolderCommand { get; }
    public ICommand SearchWebCommand { get; }
    public ICommand OpenRegistryCommand { get; }
    public ICommand RemoveEntryCommand { get; }
    public ICommand ModifyCommand { get; }
    public ICommand OpenStoreCommand { get; }
    public ICommand CopyNameCommand { get; }
    public ICommand ClearSearchCommand { get; }
    public ICommand UpdateAppCommand { get; }
    public ICommand UpdateAllCommand { get; }
    public ICommand ExportCommand { get; }

    public ProgramsViewModel(MainViewModel main)
    {
        _main = main;
        View = new ListCollectionView(Apps) { Filter = FilterApp };
        View.IsLiveSorting = true;
        View.LiveSortingProperties.Add(nameof(InstalledApp.SizeBytes));
        View.LiveSortingProperties.Add(nameof(InstalledApp.LastUsedSortKey));
        View.IsLiveFiltering = true;
        View.LiveFilteringProperties.Add(nameof(InstalledApp.SizeBytes));
        View.LiveFilteringProperties.Add(nameof(InstalledApp.IsUnused));
        View.LiveFilteringProperties.Add(nameof(InstalledApp.HasUpdate));
        if (Enum.TryParse<AppSort>(AppSettings.Current.ProgramsSort, out var saved)) _sort = saved;
        ApplySort();

        RefreshCommand = new RelayCommand(async () => await LoadAsync(), () => !IsLoading);
        UninstallCommand = new RelayCommand(async p =>
        {
            if ((p as InstalledApp ?? Selected) is { } app) await UninstallAsync(new List<InstalledApp> { app });
        }, p => (p as InstalledApp ?? Selected)?.CanUninstall == true && !IsBusy);
        UninstallCheckedCommand = new RelayCommand(async () =>
        {
            var list = Apps.Where(a => a.IsChecked && a.CanUninstall).ToList();
            if (list.Count > 0) await UninstallAsync(list);
        }, () => CheckedCount > 0 && !IsBusy);
        ForceUninstallCommand = new RelayCommand(async p =>
        {
            if ((p as InstalledApp ?? Selected) is { } app) await ForceUninstallAsync(app, null);
        }, p => (p as InstalledApp ?? Selected) is { IsStore: false, IsLauncherUninstall: false } a && (a.HasRegistryKey || a.HasLocation) && !IsBusy);
        ClearChecksCommand = new RelayCommand(() =>
        {
            foreach (var a in Apps) a.IsChecked = false;
        });
        OpenFolderCommand = new RelayCommand(p =>
        {
            if ((p as InstalledApp ?? Selected) is { HasLocation: true } a) ShellHelper.OpenFolder(a.InstallLocation);
        }, p => (p as InstalledApp ?? Selected)?.HasLocation == true);
        SearchWebCommand = new RelayCommand(p =>
        {
            if ((p as InstalledApp ?? Selected) is { } a) ShellHelper.SearchWeb($"{a.Name} что это за программа");
        });
        OpenRegistryCommand = new RelayCommand(p =>
        {
            if ((p as InstalledApp ?? Selected) is { HasRegistryKey: true } a) ShellHelper.OpenRegedit(a.RegistryFullPath);
        }, p => (p as InstalledApp ?? Selected) is { HasRegistryKey: true });
        RemoveEntryCommand = new RelayCommand(async p =>
        {
            if ((p as InstalledApp ?? Selected) is { HasRegistryKey: true } a) await RemoveEntryAsync(a);
        }, p => (p as InstalledApp ?? Selected) is { HasRegistryKey: true } && !IsBusy);
        ModifyCommand = new RelayCommand(p =>
        {
            if ((p as InstalledApp ?? Selected) is { CanModify: true } a)
            {
                try { Uninstaller.StartModify(a); }
                catch (Exception ex) { _ = _main.ShowError("Не удалось запустить установщик", ex.Message); }
            }
        }, p => (p as InstalledApp ?? Selected)?.CanModify == true);
        OpenStoreCommand = new RelayCommand(p =>
        {
            if ((p as InstalledApp ?? Selected) is { IsStore: true } a)
                ShellHelper.Open("ms-windows-store://pdp/?PFN=" + a.PackageFamilyName);
        });
        CopyNameCommand = new RelayCommand(p =>
        {
            if ((p as InstalledApp ?? Selected) is { } a)
            {
                ShellHelper.CopyText(string.IsNullOrEmpty(a.Version) ? a.Name : $"{a.Name} {a.Version}");
                _main.ShowToast("Название скопировано", "info");
            }
        });
        ClearSearchCommand = new RelayCommand(() => SearchText = "");
        UpdateAppCommand = new RelayCommand(async p =>
        {
            if ((p as InstalledApp ?? Selected) is { HasUpdate: true } a) await UpdateAsync(new List<InstalledApp> { a });
        }, p => (p as InstalledApp ?? Selected)?.HasUpdate == true && !IsBusy);
        UpdateAllCommand = new RelayCommand(async () =>
        {
            var list = Apps.Where(a => a.HasUpdate).ToList();
            if (list.Count > 0) await UpdateAsync(list);
        }, () => CountUpdates > 0 && !IsBusy);
        ExportCommand = new RelayCommand(async () => await ExportAsync(), () => Apps.Count > 0);
    }

    // ------------------------------------------------------------------ state

    private bool _isLoading;
    public bool IsLoading { get => _isLoading; set => Set(ref _isLoading, value); }

    private bool _isBusy;
    public bool IsBusy { get => _isBusy; set => Set(ref _isBusy, value); }

    private InstalledApp _selected;
    public InstalledApp Selected { get => _selected; set => Set(ref _selected, value); }

    private string _searchText = "";
    public string SearchText
    {
        get => _searchText;
        set
        {
            if (Set(ref _searchText, value ?? "")) View.Refresh();
        }
    }

    private AppFilter _filter = AppFilter.All;
    public AppFilter Filter
    {
        get => _filter;
        set
        {
            if (!Set(ref _filter, value)) return;
            View.Refresh();
            OnPropertyChanged(nameof(ShowLastUsedColumn));
            OnPropertyChanged(nameof(ShowUpdatesBar));
            OnPropertyChanged(nameof(ShowUnusedBar));
            OnPropertyChanged(nameof(IsEmptyResult));
        }
    }

    private AppSort _sort = AppSort.Size;
    public AppSort Sort
    {
        get => _sort;
        set
        {
            if (!Set(ref _sort, value)) return;
            ApplySort();
            AppSettings.Current.ProgramsSort = value.ToString();
            AppSettings.Current.Save();
            OnPropertyChanged(nameof(ShowLastUsedColumn));
        }
    }

    /// <summary>The right column shows the last launch instead of the install date.</summary>
    public bool ShowLastUsedColumn => _sort == AppSort.LastUsed || _filter == AppFilter.Unused;

    public int CountAll => Apps.Count;
    public int CountDesktop => Apps.Count(a => !a.IsStore && !a.IsGame);
    public int CountStore => Apps.Count(a => a.IsStore && !a.IsGame);
    public int CountGames => Apps.Count(a => a.IsGame);
    public int CountLarge => Apps.Count(a => a.SizeBytes >= LargeThreshold);
    public int CountRecent => Apps.Count(a => a.InstallDate >= DateTime.Now.AddDays(-30));
    public int CountUnused => Apps.Count(a => a.IsUnused);
    public int CountUpdates => Apps.Count(a => a.HasUpdate);

    public long TotalSize => Apps.Where(a => a.SizeBytes > 0).Sum(a => a.SizeBytes);
    public long UnusedSize => Apps.Where(a => a.IsUnused && a.SizeBytes > 0).Sum(a => a.SizeBytes);
    public string Summary => Apps.Count == 0
        ? "Загружаем список установленных программ…"
        : $"{Fmt.Programs(Apps.Count)} · занимают ≈ {Fmt.Size(TotalSize)}";

    public bool ShowUpdatesBar => _filter == AppFilter.Updates && CountUpdates > 0;
    public bool ShowUnusedBar => _filter == AppFilter.Unused;

    public string UnusedText => CountUnused == 0
        ? UsageReady ? "Все программы запускались недавно — или Windows не записала их запуски." : "Собираем данные о запусках…"
        : $"{Fmt.Programs(CountUnused)} не запускались дольше {MonthsText} · {Fmt.Size(UnusedSize)}. Данные {UsageSourceNote}.";

    private static string MonthsText => AppSettings.Current.UnusedMonths switch
    {
        12 => "года",
        6 => "полугода",
        int m => $"{m} {Fmt.Plural(m, "месяца", "месяцев", "месяцев")}"
    };

    private bool _isCheckingUpdates;
    public bool IsCheckingUpdates { get => _isCheckingUpdates; private set => Set(ref _isCheckingUpdates, value); }

    private bool _usageReady;
    public bool UsageReady { get => _usageReady; private set => Set(ref _usageReady, value); }

    private bool _hasPrefetch;
    public string UsageSourceNote => _hasPrefetch
        ? "по журналу запусков Windows и меню «Пуск»"
        : "по запускам из меню «Пуск», панели задач и Проводника; в режиме администратора — точнее";

    private int _checkedCount;
    public int CheckedCount { get => _checkedCount; private set => Set(ref _checkedCount, value); }

    private string _checkedText;
    public string CheckedText { get => _checkedText; private set => Set(ref _checkedText, value); }

    public bool IsEmptyResult => !IsLoading && Apps.Count > 0 && View.IsEmpty;

    private bool FilterApp(object o)
    {
        var a = (InstalledApp)o;
        if (_searchText.Length > 0 &&
            !(a.Name.Contains(_searchText, StringComparison.CurrentCultureIgnoreCase) ||
              (a.Publisher?.Contains(_searchText, StringComparison.CurrentCultureIgnoreCase) ?? false)))
            return false;
        return _filter switch
        {
            AppFilter.Desktop => !a.IsStore && !a.IsGame,
            AppFilter.Store => a.IsStore && !a.IsGame,
            AppFilter.Games => a.IsGame,
            AppFilter.Large => a.SizeBytes >= LargeThreshold,
            AppFilter.Recent => a.InstallDate >= DateTime.Now.AddDays(-30),
            AppFilter.Unused => a.IsUnused,
            AppFilter.Updates => a.HasUpdate,
            _ => true
        };
    }

    private void ApplySort()
    {
        using (View.DeferRefresh())
        {
            View.SortDescriptions.Clear();
            switch (_sort)
            {
                case AppSort.Size:
                    View.SortDescriptions.Add(new SortDescription(nameof(InstalledApp.SizeBytes), ListSortDirection.Descending));
                    break;
                case AppSort.Date:
                    View.SortDescriptions.Add(new SortDescription(nameof(InstalledApp.InstallDate), ListSortDirection.Descending));
                    break;
                case AppSort.LastUsed:
                    View.SortDescriptions.Add(new SortDescription(nameof(InstalledApp.LastUsedSortKey), ListSortDirection.Ascending));
                    break;
            }
            View.SortDescriptions.Add(new SortDescription(nameof(InstalledApp.Name), ListSortDirection.Ascending));
        }
    }

    private void RaiseCounts()
    {
        foreach (var n in new[]
                 {
                     nameof(CountAll), nameof(CountDesktop), nameof(CountStore), nameof(CountGames), nameof(CountLarge), nameof(CountRecent),
                     nameof(CountUnused), nameof(CountUpdates), nameof(TotalSize), nameof(UnusedSize), nameof(Summary), nameof(IsEmptyResult),
                     nameof(ShowUpdatesBar), nameof(UnusedText)
                 })
            OnPropertyChanged(n);
        long max = Apps.Count == 0 ? 1 : Math.Max(1, Apps.Max(a => a.SizeBytes));
        foreach (var a in Apps) a.SizeFraction = a.SizeBytes > 0 ? (double)a.SizeBytes / max : 0;
        _main.UpdateBadges();
        Changed?.Invoke();
    }

    private void UpdateChecked()
    {
        var list = Apps.Where(a => a.IsChecked).ToList();
        CheckedCount = list.Count;
        long size = list.Where(a => a.SizeBytes > 0).Sum(a => a.SizeBytes);
        CheckedText = list.Count == 0 ? "" : $"Выбрано: {Fmt.Programs(list.Count)}" + (size > 0 ? $" · {Fmt.Size(size)}" : "");
        CommandManager.InvalidateRequerySuggested();
    }

    private void OnAppChanged(object sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(InstalledApp.IsChecked)) UpdateChecked();
    }

    // ------------------------------------------------------------------ loading

    private int _loadVersion;

    public async Task LoadAsync()
    {
        IsLoading = true;
        int version = ++_loadVersion;
        string selectedId = Selected?.Id;
        try
        {
            var apps = await Task.Run(AppInventory.LoadAll);
            if (version != _loadVersion) return;
            foreach (var a in Apps) a.PropertyChanged -= OnAppChanged;
            Apps.Clear();
            foreach (var a in apps.OrderByDescending(a => a.SizeBytes))
            {
                a.PropertyChanged += OnAppChanged;
                Apps.Add(a);
            }
            Selected = Apps.FirstOrDefault(a => a.Id == selectedId);
            UpdateChecked();
            RaiseCounts();
        }
        finally
        {
            IsLoading = false;
            OnPropertyChanged(nameof(IsEmptyResult));
        }

        var dispatcher = Application.Current.Dispatcher;
        var snapshot = Apps.ToList();
        _ = Task.Run(() =>
        {
            Parallel.ForEach(snapshot, new ParallelOptions { MaxDegreeOfParallelism = 4 }, app =>
            {
                var icon = AppInventory.LoadIcon(app);
                if (icon != null) dispatcher.InvokeAsync(() => app.Icon = icon);
            });
        });
        _ = Task.Run(() =>
        {
            var locations = new HashSet<string>(
                snapshot.Where(a => a.HasLocation).Select(a => a.InstallLocation.TrimEnd('\\'))
                    .Concat(snapshot.SelectMany(a => a.ExtraLocations).Select(p => p.TrimEnd('\\'))),
                StringComparer.OrdinalIgnoreCase);
            var toMeasure = snapshot.Where(a => a.IsStore || a.IsGame || (a.SizeBytes <= 0 && a.HasLocation)).ToList();
            Parallel.ForEach(toMeasure, new ParallelOptions { MaxDegreeOfParallelism = 3 }, app =>
            {
                long size = AppInventory.MeasureSize(app, locations);
                if (size > 0)
                    dispatcher.InvokeAsync(() =>
                    {
                        app.SizeBytes = size;
                        app.SizeIsMeasured = true;
                    });
            });
            dispatcher.InvokeAsync(() =>
            {
                if (version == _loadVersion) RaiseCounts();
            });
        });
        _ = LoadUsageAsync(snapshot, version);
        _ = CheckUpdatesAsync(snapshot, version);
    }

    private async Task LoadUsageAsync(List<InstalledApp> apps, int version)
    {
        try
        {
            var (prefetch, results) = await Task.Run(() =>
            {
                var data = UsageTracker.Load();
                return (data.HasPrefetch, apps.Select(a => (a, UsageTracker.LastUsed(a, data))).ToList());
            });
            if (version != _loadVersion) return;
            _hasPrefetch = prefetch;
            foreach (var (app, last) in results) app.LastUsed = last;
            UsageReady = true;
            OnPropertyChanged(nameof(UsageSourceNote));
            RefreshUsage();
        }
        catch (Exception ex)
        {
            App.Log(ex);
        }
    }

    private async Task CheckUpdatesAsync(List<InstalledApp> apps, int version)
    {
        if (!WingetService.IsAvailable) return;
        IsCheckingUpdates = true;
        try
        {
            var upgrades = await WingetService.GetUpgradesAsync();
            if (version != _loadVersion) return;
            WingetService.Apply(apps, upgrades);
            RaiseCounts();
        }
        catch (Exception ex)
        {
            App.Log(ex);
        }
        finally
        {
            IsCheckingUpdates = false;
        }
    }

    /// <summary>Re-evaluates the "unused" filter after usage data arrived or its threshold changed.</summary>
    public void RefreshUsage()
    {
        var threshold = DateTime.Now.AddMonths(-AppSettings.Current.UnusedMonths);
        foreach (var a in Apps)
            a.IsUnused = a.LastUsed.HasValue && a.LastUsed < threshold && !(a.InstallDate > threshold) &&
                         a.Hint is not (AppHint.Dependency or AppHint.Driver or AppHint.System);
        View.Refresh();
        RaiseCounts();
    }

    public void SelectApp(InstalledApp app)
    {
        if (app == null) return;
        Filter = AppFilter.All;
        SearchText = "";
        Selected = app;
    }

    private void RemoveFromList(IEnumerable<InstalledApp> apps)
    {
        foreach (var a in apps.ToList())
        {
            a.PropertyChanged -= OnAppChanged;
            Apps.Remove(a);
            if (Selected == a) Selected = null;
        }
        UpdateChecked();
        RaiseCounts();
    }

    // ------------------------------------------------------------------ uninstall

    private enum Outcome { Removed, Kept, EntryRemoved, Abort }

    private static string LauncherHint(InstalledApp app) => app.LauncherManifest != null
        ? $"Откроется библиотека {app.LauncherName} — удалите игру там: «…» у игры → «Управление» → «Удалить»."
        : $"Игру удалит {app.LauncherName} — подтвердите удаление в его окне.";

    private async Task UninstallAsync(List<InstalledApp> apps)
    {
        if (apps.Count == 0) return;
        bool single = apps.Count == 1;
        bool anyQuiet = apps.Any(a => !a.IsStore && a.HasQuiet);
        long size = apps.Where(a => a.SizeBytes > 0).Sum(a => a.SizeBytes);
        var s = AppSettings.Current;

        string message = single
            ? apps[0].IsStore
                ? "Приложение Microsoft Store будет удалено вместе с его данными. Если оно запущено, Windows его закроет."
                : apps[0].IsLauncherUninstall
                    ? LauncherHint(apps[0])
                    : "Запустится штатный деинсталлятор программы — следуйте его инструкциям."
            : "Программы будут удаляться по очереди. Для классических программ откроются их деинсталляторы.";
        if (size > 0) message += $"\nОсвободится примерно {Fmt.Size(size)}.";
        if (apps.Any(a => !a.IsSteamGame && a.Name == "Steam") && Apps.Any(a => a.IsSteamGame))
            message += "\n\nВнимание: деинсталлятор Steam удалит и все игры из своей папки.";
        var risky = apps.Where(a => a.Hint is AppHint.Dependency or AppHint.Driver or AppHint.System).ToList();
        if (risky.Count > 0)
            message += single
                ? $"\n\nОсторожно: {AppAdvisor.Explanation(risky[0].Hint)}"
                : $"\n\nОсторожно: {string.Join(", ", risky.Take(3).Select(a => $"«{a.Name}»"))}{(risky.Count > 3 ? " и др." : "")} — " +
                  "драйверы или компоненты, которые нужны другим программам.";

        bool admin = ShellHelper.IsAdmin;
        var confirm = new DialogViewModel
        {
            Title = single ? $"Удалить «{apps[0].Name}»?" : $"Удалить {Fmt.Programs(apps.Count)}?",
            Message = message,
            Glyph = "",
            Kind = "danger",
            IsDanger = true,
            PrimaryText = "Удалить",
            SecondaryText = "Отмена",
            CheckText = apps.Any(a => !a.IsStore && !a.IsLauncherUninstall) ? "Найти остатки после удаления" : null,
            CheckValue = s.SearchLeftovers,
            Check2Text = anyQuiet ? "Тихий режим, если программа его поддерживает" : null,
            Check3Text = apps.Any(a => !a.IsStore && !a.IsLauncherUninstall)
                ? admin ? "Создать точку восстановления Windows" : "Точка восстановления (нужен режим администратора)"
                : null,
            Check3Enabled = admin,
            Check3Value = admin && s.RestorePointBeforeUninstall,
            Items = single ? null : new ObservableCollection<SelectableItem>(apps.Select(a => new SelectableItem
            {
                Title = a.Name, Subtitle = a.HintLabel ?? a.Publisher, Detail = a.SizeText, Tag = a, IsChecked = true
            }))
        };
        if (!await _main.ShowDialog(confirm)) return;
        if (!single) apps = confirm.Items.Where(i => i.IsChecked).Select(i => (InstalledApp)i.Tag).ToList();
        if (apps.Count == 0) return;

        IsBusy = true;
        var removed = new List<InstalledApp>();
        var entriesRemoved = new List<InstalledApp>();
        try
        {
            if (confirm.Check3Value && admin && apps.Any(a => !a.IsStore && !a.IsLauncherUninstall))
                await CreateRestorePointAsync(single ? $"Удаление {apps[0].Name}" : $"Удаление {apps.Count} программ");

            for (int i = 0; i < apps.Count; i++)
            {
                var outcome = await UninstallOneAsync(apps[i], confirm.Check2Value, i, apps.Count);
                if (outcome == Outcome.Removed)
                {
                    removed.Add(apps[i]);
                    ActionJournal.Add("uninstall", $"Удалена «{apps[i].Name}»",
                        string.Join(" · ", new[] { apps[i].Publisher, apps[i].Version }.Where(x => !string.IsNullOrWhiteSpace(x))),
                        Math.Max(0, apps[i].SizeBytes));
                }
                else if (outcome == Outcome.EntryRemoved) entriesRemoved.Add(apps[i]);
                else if (outcome == Outcome.Abort) break;
            }
            RemoveFromList(removed.Concat(entriesRemoved));

            if (removed.Count > 0)
                _main.ShowToast(removed.Count == 1 ? $"«{removed[0].Name}» удалена" : $"Удалено: {Fmt.Programs(removed.Count)}");

            var forLeftovers = removed.Where(a => !a.IsStore && !a.IsLauncherUninstall && !_forced.Contains(a)).ToList();
            if (confirm.CheckValue && forLeftovers.Count > 0)
                await OfferLeftoversAsync(forLeftovers);
        }
        finally
        {
            _forced.Clear();
            IsBusy = false;
            CommandManager.InvalidateRequerySuggested();
            _main.Dashboard.Refresh();
        }
    }

    /// <summary>Programs removed by the forced removal during this run: their leftovers are already gone.</summary>
    private readonly HashSet<InstalledApp> _forced = new();

    private async Task CreateRestorePointAsync(string description)
    {
        var dlg = new DialogViewModel
        {
            Title = "Создаём точку восстановления…",
            Message = "Это займёт до минуты. Если удаление что-то сломает, систему можно будет вернуть к этому состоянию.",
            Glyph = "", IsBusy = true, PrimaryText = null, ShowProgress = true, IsIndeterminate = true
        };
        _ = _main.ShowDialog(dlg);
        var (outcome, message) = await Task.Run(() => RestorePointService.Create("Простор: " + description));
        dlg.Close(outcome != RestorePointOutcome.Failed);
        switch (outcome)
        {
            case RestorePointOutcome.Created:
                _main.ShowToast("Точка восстановления создана", "info");
                break;
            case RestorePointOutcome.Recent:
                _main.ShowToast(message, "info");
                break;
            default:
                _main.ShowToast("Точка восстановления не создана: " + message, "warning");
                await Task.Delay(1200);
                break;
        }
    }

    private static DialogViewModel NewProgressDialog(InstalledApp app, int index, int total) => new()
    {
        Title = $"Удаляем «{app.Name}»",
        Glyph = "",
        IsBusy = true,
        ShowProgress = true,
        IsIndeterminate = true,
        PrimaryText = null,
        ShowOverall = total > 1,
        OverallProgress = (double)index / total,
        OverallText = $"Программа {index + 1} из {total}"
    };

    private async Task<Outcome> UninstallOneAsync(InstalledApp app, bool quiet, int index, int total)
    {
        if (app.IsStore) return await UninstallStoreAsync(app, index, total);

        while (true)
        {
            var dlg = NewProgressDialog(app, index, total);
            dlg.Message = app.IsLauncherUninstall ? $"Открываем {app.LauncherName}…" : "Запускаем деинсталлятор…";
            dlg.SecondaryText = total > 1 ? "Пропустить" : "Прекратить ожидание";
            _ = _main.ShowDialog(dlg);

            Process process;
            try
            {
                process = await Task.Run(() => Uninstaller.Start(app, quiet));
            }
            catch (Win32Exception ex) when (ex.NativeErrorCode == 1223)
            {
                dlg.Close(false);
                _main.ShowToast("Удаление отменено", "warning");
                return Outcome.Kept;
            }
            catch (Win32Exception ex) when (ex.NativeErrorCode is 2 or 3)
            {
                dlg.Close(false);
                return await OfferBrokenAsync(app);
            }
            catch (Exception ex)
            {
                dlg.Close(false);
                await _main.ShowError($"Не удалось запустить деинсталлятор «{app.Name}»", ex.Message);
                return Outcome.Kept;
            }

            bool gone = await WaitForUninstallAsync(app, process, dlg);
            dlg.Close(gone);
            if (gone) return Outcome.Removed;

            // The uninstaller finished (or the user stopped waiting) but the program is still registered.
            bool orphan = Uninstaller.IsUninstallerMissing(app) || (app.HasLocation && !Directory.Exists(app.InstallLocation));
            bool canForce = !app.IsLauncherUninstall && (app.HasRegistryKey || app.HasLocation);
            var ask = new DialogViewModel
            {
                Title = $"«{app.Name}» не удалена",
                Message = orphan
                    ? "Программа осталась в списке, хотя её файлов уже нет. Скорее всего, это «осиротевшая» запись — её можно убрать из списка."
                    : "Деинсталлятор завершил работу, но программа осталась в системе. Возможно, удаление было отменено или завершилось с ошибкой." +
                      (canForce ? " Если деинсталлятор не работает, можно удалить программу принудительно." : ""),
                Glyph = "",
                Kind = "warning",
                PrimaryText = "Повторить",
                SecondaryText = total > 1 ? "Пропустить" : "Закрыть",
                TertiaryText = orphan && app.HasRegistryKey ? "Убрать из списка" : canForce ? "Удалить принудительно" : null
            };
            await _main.ShowDialog(ask);
            switch (ask.Choice)
            {
                case "primary":
                    continue;
                case "tertiary":
                    return orphan ? await OfferRemoveEntryAsync(app, missingUninstaller: false) : await ForceUninstallAsync(app, index);
                default:
                    return Outcome.Kept;
            }
        }
    }

    /// <summary>
    /// Waits for the uninstaller and all processes it spawns. Never waits forever: once the uninstaller is gone the result
    /// is decided after a short grace period (longer when the work was handed over to a launcher such as Steam).
    /// </summary>
    private static async Task<bool> WaitForUninstallAsync(InstalledApp app, Process process, DialogViewModel dlg)
    {
        var tracker = process != null ? new ProcessTracker(process.Id) : null;
        process?.Dispose();
        var stop = dlg.Result; // completes when the user presses "Прекратить ожидание" / "Пропустить"
        var sw = Stopwatch.StartNew();
        var lastAlive = TimeSpan.Zero;
        bool launcher = app.IsLauncherUninstall;

        while (!stop.IsCompleted)
        {
            var (installed, alive, name) = await Task.Run(() =>
            {
                bool inst = Uninstaller.IsStillInstalled(app);
                bool al = tracker?.Poll() ?? false;
                if (al) tracker.RaiseWindows();
                return (inst, al, tracker?.ActiveName);
            });
            if (!installed)
            {
                dlg.IsIndeterminate = false;
                dlg.Progress = 1;
                dlg.Message = "Готово — программа удалена.";
                await Task.Delay(350);
                return true;
            }
            if (alive) lastAlive = sw.Elapsed;

            var idle = sw.Elapsed - lastAlive;
            bool handedOff = launcher || lastAlive < TimeSpan.FromSeconds(4); // exited almost at once → work continues elsewhere
            var limit = launcher ? TimeSpan.FromMinutes(5) : handedOff ? TimeSpan.FromSeconds(20) : TimeSpan.FromSeconds(8);

            if (launcher)
                dlg.Message = LauncherHint(app) + " Это окно закроется само.";
            else if (alive)
                dlg.Message = "Идёт удаление — следуйте инструкциям в окне деинсталлятора. Это окно закроется само.";
            else if (handedOff)
                dlg.Message = "Деинсталлятор сразу закрылся. Если удаление продолжается в другом окне — дождитесь его, если вы его отменили — нажмите «Не ждать».";
            else
                dlg.Message = "Деинсталлятор закрылся — проверяем результат…";
            if (!alive && !dlg.ShowOverall) dlg.SecondaryText = "Не ждать";

            string text = $"Прошло {Fmt.Clock(sw.Elapsed)}";
            if (alive && !string.IsNullOrEmpty(name)) text += $"  ·  процесс {name}";
            if (!alive && idle > TimeSpan.FromSeconds(1)) text += $"  ·  ожидание закончится через {Fmt.Clock(limit - idle)}";
            dlg.ProgressText = text;

            if (!alive && idle > limit) break;
            await Task.WhenAny(stop, Task.Delay(700));
        }

        dlg.Message = "Проверяем результат…";
        return !await Task.Run(() => Uninstaller.IsStillInstalled(app));
    }

    private async Task<Outcome> UninstallStoreAsync(InstalledApp app, int index, int total)
    {
        var dlg = NewProgressDialog(app, index, total);
        dlg.Message = "Удаляем приложение Microsoft Store…";
        dlg.IsIndeterminate = false;
        dlg.ProgressText = "Подготовка…";
        dlg.SecondaryText = "Отменить";
        _ = _main.ShowDialog(dlg);

        using var cts = new CancellationTokenSource();
        _ = dlg.Result.ContinueWith(_ =>
        {
            if (dlg.Choice == "secondary") cts.Cancel();
        }, TaskScheduler.Default);

        var progress = new Progress<double>(p =>
        {
            dlg.Progress = p;
            dlg.ProgressText = p >= 1 ? "Завершаем…" : $"Выполнено {Fmt.Percent(p)}";
        });
        try
        {
            await Uninstaller.RemoveStoreAppAsync(app, progress, cts.Token);
            dlg.Progress = 1;
            dlg.ProgressText = "Выполнено 100%";
            dlg.Message = "Готово — приложение удалено.";
            await Task.Delay(350);
            dlg.Close(true);
            return Outcome.Removed;
        }
        catch (OperationCanceledException)
        {
            dlg.Close(false);
            bool gone = await Task.Run(() => !Uninstaller.IsStillInstalled(app));
            if (gone) return Outcome.Removed;
            _main.ShowToast("Удаление отменено", "warning");
            return Outcome.Kept;
        }
        catch (Exception ex)
        {
            dlg.Close(false);
            await _main.ShowError($"Не удалось удалить «{app.Name}»", ex.Message);
            return Outcome.Kept;
        }
    }

    /// <summary>The uninstaller file is missing: offer to force-remove the rest or to drop the entry.</summary>
    private async Task<Outcome> OfferBrokenAsync(InstalledApp app)
    {
        bool hasFiles = app.HasLocation && Directory.Exists(app.InstallLocation);
        if (!hasFiles) return await OfferRemoveEntryAsync(app, missingUninstaller: true);
        var ask = new DialogViewModel
        {
            Title = "Деинсталлятор не найден",
            Message = $"Файл деинсталлятора «{app.Name}» отсутствует, но папка программы на месте. «Простор» может удалить программу сам: " +
                      "папку, записи в реестре, ярлыки и задачи — с резервными копиями.",
            Glyph = "",
            Kind = "warning",
            PrimaryText = "Удалить принудительно",
            SecondaryText = "Отмена",
            TertiaryText = app.HasRegistryKey ? "Убрать из списка" : null
        };
        await _main.ShowDialog(ask);
        return ask.Choice switch
        {
            "primary" => await ForceUninstallAsync(app, 0, confirmed: true),
            "tertiary" => await OfferRemoveEntryAsync(app, missingUninstaller: false),
            _ => Outcome.Kept
        };
    }

    private async Task<Outcome> OfferRemoveEntryAsync(InstalledApp app, bool missingUninstaller)
    {
        if (!app.HasRegistryKey)
        {
            await _main.ShowError($"Не удалось удалить «{app.Name}»", "Деинсталлятор программы не найден.");
            return Outcome.Kept;
        }
        var ask = new DialogViewModel
        {
            Title = missingUninstaller ? "Деинсталлятор не найден" : $"Убрать «{app.Name}» из списка?",
            Message = (missingUninstaller
                          ? $"Файл деинсталлятора «{app.Name}» отсутствует — похоже, программу уже удалили вручную, а в списке осталась её запись.\n\n"
                          : "")
                      + "Будет удалена только запись в реестре, файлы на диске не трогаются. Резервная копия записи сохранится — её можно вернуть из «Журнала».",
            Glyph = "",
            Kind = "warning",
            PrimaryText = "Убрать из списка",
            SecondaryText = "Отмена"
        };
        if (!await _main.ShowDialog(ask)) return Outcome.Kept;
        try
        {
            string backup = await Task.Run(() => RegistryCleaner.RemoveUninstallEntry(app));
            ActionJournal.Add("entry", $"Убрана запись «{app.Name}»", "Только запись в списке программ, файлы не тронуты", 0,
                backup != null ? UndoKind.RegImport : UndoKind.None, backup != null ? new[] { backup } : null);
            _main.ShowToast($"Запись «{app.Name}» убрана из списка");
            return Outcome.EntryRemoved;
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException)
        {
            await _main.ShowError("Нужны права администратора",
                "Эта запись относится ко всем пользователям компьютера. Перезапустите «Простор» от имени администратора и повторите.");
            return Outcome.Kept;
        }
        catch (Exception ex)
        {
            await _main.ShowError("Не удалось убрать запись", ex.Message);
            return Outcome.Kept;
        }
    }

    private async Task RemoveEntryAsync(InstalledApp app)
    {
        if (await OfferRemoveEntryAsync(app, missingUninstaller: false) != Outcome.EntryRemoved) return;
        RemoveFromList(new[] { app });
    }

    // ------------------------------------------------------------------ forced removal & leftovers

    /// <param name="index">Set when called from a running uninstall (null when started from the menu).</param>
    private async Task<Outcome> ForceUninstallAsync(InstalledApp app, int? index, bool confirmed = false)
    {
        bool standalone = index == null;
        if (!confirmed)
        {
            var ask = new DialogViewModel
            {
                Title = $"Удалить «{app.Name}» принудительно?",
                Message = "Деинсталлятор программы запускаться не будет — «Простор» сам удалит её папку, записи в реестре, ярлыки, задачи и службы. " +
                          "Используйте, если обычное удаление не работает.\n\nПапки и ярлыки попадут в корзину, записи реестра сохранятся в резервных копиях. " +
                          "Запущенные процессы программы будут закрыты.",
                Glyph = "",
                Kind = "danger",
                IsDanger = true,
                PrimaryText = "Найти, что удалить",
                SecondaryText = "Отмена"
            };
            if (!await _main.ShowDialog(ask)) return Outcome.Kept;
        }
        if (standalone) IsBusy = true;
        try
        {
            var search = new DialogViewModel
            {
                Title = $"Ищем всё, что относится к «{app.Name}»…", Glyph = "", IsBusy = true, PrimaryText = null,
                ShowProgress = true, IsIndeterminate = true
            };
            _ = _main.ShowDialog(search);
            var remaining = Apps.ToList();
            var found = await Task.Run(() => LeftoverFinder.Find(app, remaining, includeInstallFolder: true));
            search.Close(true);

            bool ok = await ReviewAndRemoveAsync(new List<(InstalledApp, List<Leftover>)> { (app, found) },
                $"Удаление «{app.Name}»", "Отмеченное будет удалено. Папки и ярлыки — в корзину, записи реестра и задачи — с резервными копиями.",
                "Удалить", force: true);
            if (!ok) return Outcome.Kept;

            if (app.HasRegistryKey)
            {
                try
                {
                    string backup = await Task.Run(() => RegistryCleaner.RemoveUninstallEntry(app));
                    if (backup != null)
                        ActionJournal.Add("entry", $"Убрана запись «{app.Name}»", "При принудительном удалении", 0, UndoKind.RegImport, new[] { backup });
                }
                catch (Exception ex)
                {
                    await _main.ShowError("Запись программы не удалена", ex is UnauthorizedAccessException or System.Security.SecurityException
                        ? "Для записей «для всех пользователей» нужен режим администратора."
                        : ex.Message);
                    return Outcome.Kept;
                }
            }
            _forced.Add(app);
            if (standalone)
            {
                ActionJournal.Add("force", $"Принудительно удалена «{app.Name}»", app.InstallLocation, Math.Max(0, app.SizeBytes));
                RemoveFromList(new[] { app });
                _main.ShowToast($"«{app.Name}» удалена");
                _main.Dashboard.Refresh();
            }
            return Outcome.Removed;
        }
        finally
        {
            if (standalone) IsBusy = false;
        }
    }

    private async Task OfferLeftoversAsync(List<InstalledApp> removed)
    {
        var search = new DialogViewModel
        {
            Title = "Ищем остатки…",
            Message = "Проверяем папки, реестр, ярлыки, автозагрузку, задачи и службы.",
            Glyph = "",
            IsBusy = true,
            ShowProgress = true,
            IsIndeterminate = true,
            PrimaryText = null
        };
        _ = _main.ShowDialog(search);
        var remaining = Apps.ToList();
        var found = await Task.Run(() => removed.Select(a => (a, LeftoverFinder.Find(a, remaining))).ToList());
        search.Close(true);
        if (found.All(f => f.Item2.Count == 0))
        {
            _main.ShowToast("Остатков не найдено — всё чисто", "success");
            return;
        }
        await ReviewAndRemoveAsync(found, "Найдены остатки",
            "После удаления остались файлы и записи. Отмеченное будет удалено: папки и ярлыки — в корзину, записи реестра и задачи — с резервными копиями.",
            "Удалить остатки", force: false);
    }

    /// <summary>Shows the found items for review and removes the checked ones. Returns false when cancelled.</summary>
    private async Task<bool> ReviewAndRemoveAsync(List<(InstalledApp app, List<Leftover> items)> found, string title, string message,
        string primary, bool force)
    {
        bool admin = ShellHelper.IsAdmin;
        var all = found.SelectMany(f => f.items).ToList();
        var allowed = all.Where(i => admin || !i.NeedsAdmin).ToList();
        int hidden = all.Count - allowed.Count;
        if (allowed.Count == 0)
        {
            if (hidden > 0)
                await _main.ShowError("Нужны права администратора",
                    $"Найдено {Fmt.Count(hidden, "запись", "записи", "записей")}, которые можно удалить только в режиме администратора.");
            else if (!force) _main.ShowToast("Остатков не найдено — всё чисто", "success");
            return force && hidden == 0;
        }

        long total = allowed.Where(i => i.Kind == LeftoverKind.Folder).Sum(i => i.Size);
        var dlg = new DialogViewModel
        {
            Title = title,
            Message = message + (total > 0 ? $"\nПапки занимают {Fmt.Size(total)}." : ""),
            Glyph = force ? "" : "",
            Kind = force ? "danger" : "warning",
            IsDanger = force,
            PrimaryText = primary,
            SecondaryText = force ? "Отмена" : "Оставить",
            Footnote = hidden > 0
                ? $"Ещё {Fmt.Count(hidden, "запись", "записи", "записей")} (службы, правила брандмауэра, записи для всех пользователей) можно удалить в режиме администратора."
                : null,
            Items = new ObservableCollection<SelectableItem>(allowed.Select(i => i.ToItem()))
        };
        if (!await _main.ShowDialog(dlg)) return false;
        var chosen = dlg.Items.Where(i => i.IsChecked).Select(i => (Leftover)i.Tag).ToList();
        if (chosen.Count == 0) return force;

        if (force)
        {
            foreach (var p in LeftoverFinder.RunningFrom(found[0].app.InstallLocation))
            {
                try
                {
                    p.Kill(true);
                    p.WaitForExit(3000);
                }
                catch { }
                finally
                {
                    p.Dispose();
                }
            }
        }

        var busy = new DialogViewModel
        {
            Title = "Удаляем…", Glyph = "", IsBusy = true, ShowProgress = true, IsIndeterminate = true, PrimaryText = null
        };
        _ = _main.ShowDialog(busy);
        string label = found.Count == 1 ? found[0].app.Name : "leftovers";
        var result = await Task.Run(() => LeftoverFinder.Remove(chosen, label, new Progress<string>(t => busy.ProgressText = t)));
        busy.Close(true);

        string names = found.Count == 1 ? $"«{found[0].app.Name}»" : Fmt.Programs(found.Count);
        if (result.Recycled.Count > 0)
            ActionJournal.Add("leftovers", $"Остатки {names} перемещены в корзину", Fmt.Count(result.Recycled.Count, "объект", "объекта", "объектов"),
                result.Bytes, UndoKind.RecycleRestore, result.Recycled);
        if (result.RegBackups.Count > 0)
            ActionJournal.Add("leftovers", $"Удалены записи реестра {names}", $"{Fmt.Count(result.RegBackups.Count, "ключ", "ключа", "ключей")} · есть резервные копии",
                0, UndoKind.RegImport, result.RegBackups);
        if (result.TaskBackups.Count > 0)
            ActionJournal.Add("leftovers", $"Удалены задачи планировщика {names}", string.Join(", ", result.TaskBackups.Select(t => t.Split('|')[0])),
                0, UndoKind.TaskRestore, result.TaskBackups);

        _main.ShowToast(result.Failed == 0
                ? $"Удалено: {Fmt.Count(result.Removed, "объект", "объекта", "объектов")}"
                : $"Удалено {result.Removed} из {result.Removed + result.Failed} — часть занята или требует прав администратора",
            result.Failed == 0 ? "success" : "warning");
        _ = _main.Cleanup.ScanAsync();
        return true;
    }

    // ------------------------------------------------------------------ updates

    private async Task UpdateAsync(List<InstalledApp> apps)
    {
        if (!WingetService.IsAvailable)
        {
            await _main.ShowError("winget не найден", "Обновление работает через «Установщик приложений» (winget) из Microsoft Store.");
            return;
        }
        if (apps.Count > 1)
        {
            var confirm = new DialogViewModel
            {
                Title = $"Обновить {Fmt.Programs(apps.Count)}?",
                Message = "Обновления скачаются и установятся по очереди. Некоторым программам понадобится подтверждение администратора, " +
                          "а запущенные программы могут закрыться.",
                Glyph = "",
                PrimaryText = "Обновить",
                SecondaryText = "Отмена",
                Items = new ObservableCollection<SelectableItem>(apps.Select(a => new SelectableItem
                {
                    Title = a.Name, Subtitle = $"{a.Version} → {a.UpdateVersion}", Detail = a.SizeText, Tag = a, IsChecked = true
                }))
            };
            if (!await _main.ShowDialog(confirm)) return;
            apps = confirm.Items.Where(i => i.IsChecked).Select(i => (InstalledApp)i.Tag).ToList();
            if (apps.Count == 0) return;
        }

        IsBusy = true;
        int done = 0, failed = 0;
        try
        {
            for (int i = 0; i < apps.Count; i++)
            {
                var app = apps[i];
                var dlg = new DialogViewModel
                {
                    Title = $"Обновляем «{app.Name}»",
                    Message = $"Версия {app.Version} → {app.UpdateVersion}",
                    Glyph = "", IsBusy = true, ShowProgress = true, IsIndeterminate = true, PrimaryText = null,
                    ProgressText = "Скачиваем…",
                    SecondaryText = apps.Count > 1 ? "Прервать" : null,
                    ShowOverall = apps.Count > 1, OverallProgress = (double)i / apps.Count, OverallText = $"Программа {i + 1} из {apps.Count}"
                };
                _ = _main.ShowDialog(dlg);
                using var cts = new CancellationTokenSource();
                _ = dlg.Result.ContinueWith(_ =>
                {
                    if (dlg.Choice == "secondary") cts.Cancel();
                }, TaskScheduler.Default);
                CmdResult r;
                try
                {
                    r = await WingetService.UpgradeAsync(app.WingetId, new Progress<(double? p, string line)>(x =>
                    {
                        if (x.p is { } p)
                        {
                            dlg.IsIndeterminate = false;
                            dlg.Progress = p;
                        }
                        if (!string.IsNullOrWhiteSpace(x.line)) dlg.ProgressText = x.line;
                    }), cts.Token);
                }
                catch (OperationCanceledException)
                {
                    dlg.Close(false);
                    break;
                }
                dlg.Close(true);
                if (WingetService.IsSuccess(r))
                {
                    done++;
                    ActionJournal.Add("update", $"Обновлена «{app.Name}»", $"{app.Version} → {app.UpdateVersion}");
                    app.UpdateVersion = null;
                }
                else
                {
                    failed++;
                    App.Log(new Exception($"winget upgrade {app.WingetId}: {r.ExitCode}\n{r.Output}"));
                }
            }
        }
        finally
        {
            IsBusy = false;
            RaiseCounts();
        }
        if (failed == 0 && done > 0) _main.ShowToast(done == 1 ? $"«{apps[0].Name}» обновлена" : $"Обновлено: {Fmt.Programs(done)}");
        else if (failed > 0)
            await _main.ShowError(done == 0 ? "Обновление не удалось" : $"Обновлено {done} из {done + failed}",
                "Установщик вернул ошибку — возможно, программа запущена или обновление требует подтверждения. Закройте её и повторите.");
        if (done > 0) _ = LoadAsync();
    }

    // ------------------------------------------------------------------ export

    private async Task ExportAsync()
    {
        var ask = new DialogViewModel
        {
            Title = "Экспорт списка программ",
            Message = "Таблица откроется в Excel. Список для winget поможет после переустановки Windows поставить всё одной командой:\n" +
                      "winget import -i файл.json",
            Glyph = "",
            PrimaryText = "Таблица CSV",
            SecondaryText = "Отмена",
            TertiaryText = WingetService.IsAvailable ? "Список для winget" : null
        };
        await _main.ShowDialog(ask);
        if (ask.Choice is not ("primary" or "tertiary")) return;
        bool winget = ask.Choice == "tertiary";

        var save = new Microsoft.Win32.SaveFileDialog
        {
            FileName = winget ? $"programs-{DateTime.Now:yyyy-MM-dd}.json" : $"Программы {Environment.MachineName} {DateTime.Now:yyyy-MM-dd}.csv",
            Filter = winget ? "Список winget (*.json)|*.json" : "Таблица CSV (*.csv)|*.csv",
            InitialDirectory = Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments)
        };
        if (save.ShowDialog() != true) return;
        try
        {
            if (winget)
            {
                var busy = new DialogViewModel { Title = "Собираем список для winget…", Glyph = "", IsBusy = true, ShowProgress = true, PrimaryText = null };
                _ = _main.ShowDialog(busy);
                var r = await WingetService.ExportAsync(save.FileName);
                busy.Close(r.Ok);
                if (!File.Exists(save.FileName)) throw new IOException(r.Output.Trim());
            }
            else
            {
                await File.WriteAllTextAsync(save.FileName, BuildCsv(Apps), new UTF8Encoding(true));
            }
            ShellHelper.ShowInExplorer(save.FileName);
            _main.ShowToast("Список сохранён");
        }
        catch (Exception ex)
        {
            await _main.ShowError("Не удалось сохранить список", ex.Message);
        }
    }

    public static string BuildCsv(IEnumerable<InstalledApp> apps)
    {
        static string Q(string s)
        {
            s ??= "";
            return s.IndexOfAny(new[] { ';', '"', '\n', '\r' }) >= 0 ? "\"" + s.Replace("\"", "\"\"") + "\"" : s;
        }
        var sb = new StringBuilder();
        sb.AppendLine("Название;Издатель;Версия;Размер, МБ;Установлена;Последний запуск;Тип;Пометка;Доступно обновление;Папка");
        foreach (var a in apps.OrderBy(a => a.Name, StringComparer.CurrentCultureIgnoreCase))
        {
            sb.Append(Q(a.Name)).Append(';')
                .Append(Q(a.Publisher)).Append(';')
                .Append(Q(a.Version)).Append(';')
                .Append(a.SizeBytes > 0 ? Math.Round(a.SizeBytes / 1048576.0, 1).ToString(Fmt.Ru) : "").Append(';')
                .Append(a.InstallDate?.ToString("dd.MM.yyyy") ?? "").Append(';')
                .Append(a.LastUsed?.ToString("dd.MM.yyyy") ?? "").Append(';')
                .Append(Q(a.KindText)).Append(';')
                .Append(Q(a.HintLabel)).Append(';')
                .Append(Q(a.UpdateVersion)).Append(';')
                .Append(Q(a.InstallLocation)).AppendLine();
        }
        return sb.ToString();
    }
}
