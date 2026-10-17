using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Windows;
using System.Windows.Data;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;

namespace Prostor.ViewModels;

public enum StartupFilter { All, Programs, Tasks, Services }

public sealed class StartupViewModel : ObservableObject
{
    private readonly MainViewModel _main;
    private bool _suppress;

    public ObservableCollection<StartupItem> Items { get; } = new();
    public ListCollectionView View { get; }

    public ICommand RefreshCommand { get; }
    public ICommand OpenLocationCommand { get; }
    public ICommand SearchWebCommand { get; }
    public ICommand ToggleDelayCommand { get; }

    public event Action Changed;

    public StartupViewModel(MainViewModel main)
    {
        _main = main;
        View = new ListCollectionView(Items) { Filter = FilterItem };
        View.GroupDescriptions.Add(new PropertyGroupDescription(nameof(StartupItem.GroupName)));

        RefreshCommand = new RelayCommand(async () => await LoadAsync());
        OpenLocationCommand = new RelayCommand(p =>
        {
            if (p is StartupItem s && !string.IsNullOrEmpty(s.FilePath)) ShellHelper.ShowInExplorer(s.FilePath);
        }, p => p is StartupItem { FilePath: not null });
        SearchWebCommand = new RelayCommand(p =>
        {
            if (p is StartupItem s) ShellHelper.SearchWeb($"{s.Name} в автозагрузке что это");
        });
        ToggleDelayCommand = new RelayCommand(async p =>
        {
            if (p is StartupItem { CanDelay: true } s) await ToggleDelayAsync(s);
        }, p => p is StartupItem { CanDelay: true, CanModify: true });
    }

    private bool _isLoading;
    public bool IsLoading { get => _isLoading; set => Set(ref _isLoading, value); }

    private string _searchText = "";
    public string SearchText
    {
        get => _searchText;
        set
        {
            if (Set(ref _searchText, value ?? "")) View.Refresh();
        }
    }

    private StartupFilter _filter = StartupFilter.All;
    public StartupFilter Filter
    {
        get => _filter;
        set
        {
            if (Set(ref _filter, value)) View.Refresh();
        }
    }

    private bool FilterItem(object o)
    {
        var i = (StartupItem)o;
        if (_searchText.Length > 0 &&
            !(i.Title.Contains(_searchText, StringComparison.CurrentCultureIgnoreCase) ||
              i.Name.Contains(_searchText, StringComparison.CurrentCultureIgnoreCase) ||
              (i.Publisher?.Contains(_searchText, StringComparison.CurrentCultureIgnoreCase) ?? false)))
            return false;
        return _filter switch
        {
            StartupFilter.Programs => i.Kind is StartupKind.Run or StartupKind.Folder,
            StartupFilter.Tasks => i.Kind == StartupKind.Task,
            StartupFilter.Services => i.Kind == StartupKind.Service,
            _ => true
        };
    }

    public int EnabledCount => Items.Count(i => i.IsEnabled);
    public int CountPrograms => Items.Count(i => i.Kind is StartupKind.Run or StartupKind.Folder);
    public int CountTasks => Items.Count(i => i.Kind == StartupKind.Task);
    public int CountServices => Items.Count(i => i.Kind == StartupKind.Service);

    public string Summary => Items.Count == 0
        ? IsLoading ? "Собираем список…" : "Записей автозагрузки не найдено"
        : $"Включено {EnabledCount} из {Items.Count}: программы, задачи планировщика и сторонние службы";

    // ------------------------------------------------------------------ boot time

    private List<double> _bootValues;
    public List<double> BootValues { get => _bootValues; private set => Set(ref _bootValues, value); }

    private List<string> _bootLabels;
    public List<string> BootLabels { get => _bootLabels; private set => Set(ref _bootLabels, value); }

    private string _bootTitle = "—";
    public string BootTitle { get => _bootTitle; private set => Set(ref _bootTitle, value); }

    private string _bootNote;
    public string BootNote { get => _bootNote; private set => Set(ref _bootNote, value); }

    public string UptimeText
    {
        get
        {
            var up = TimeSpan.FromMilliseconds(Environment.TickCount64);
            var since = DateTime.Now - up;
            return $"Компьютер включён {Fmt.Date(since)} в {since:HH:mm} · работает {Fmt.Duration(up)}";
        }
    }

    private async Task LoadBootAsync()
    {
        var boots = await Task.Run(() => StartupService.LoadBootHistory());
        OnPropertyChanged(nameof(UptimeText));
        if (boots == null)
        {
            BootValues = null;
            BootTitle = "—";
            BootNote = "Windows открывает журнал загрузки только администраторам — перезапустите «Простор» от имени администратора.";
            return;
        }
        if (boots.Count == 0)
        {
            BootValues = null;
            BootTitle = "—";
            BootNote = "Windows ещё не записала ни одной загрузки. Данные появятся после перезагрузки компьютера.";
            return;
        }
        var last = boots[^1];
        BootValues = boots.Select(b => b.MainPathSeconds).ToList();
        BootLabels = boots.Select(b => $"{Fmt.Date(b.Time)} {b.Time:HH:mm} — {Seconds(b.MainPathSeconds)} до рабочего стола, " +
                                       $"{Seconds(b.TotalSeconds)} вместе с автозагрузкой ({b.StartupApps} прогр.)").ToList();
        BootTitle = Seconds(last.MainPathSeconds);
        double avg = boots.Average(b => b.MainPathSeconds);
        string note = $"до рабочего стола при последней загрузке · в среднем {Seconds(avg)}; с программами автозагрузки — {Seconds(last.TotalSeconds)}";
        if ((DateTime.Now - last.Time).TotalDays > 30)
            note += $". Последняя запись — {Fmt.Date(last.Time)}: при включённом «быстром запуске» Windows записывает только полные перезагрузки.";
        BootNote = note;
    }

    private static string Seconds(double s) => s >= 60 ? $"{(int)(s / 60)} мин {(int)(s % 60)} с" : $"{s:0.#} с";

    // ------------------------------------------------------------------ loading

    public async Task LoadAsync()
    {
        IsLoading = true;
        OnPropertyChanged(nameof(Summary));
        _ = LoadBootAsync();
        try
        {
            var items = await Task.Run(StartupService.Load);
            foreach (var i in Items) i.PropertyChanged -= OnItemChanged;
            Items.Clear();
            foreach (var i in items)
            {
                i.PropertyChanged += OnItemChanged;
                Items.Add(i);
            }
            Raise();
            var dispatcher = Application.Current.Dispatcher;
            _ = Task.Run(() =>
            {
                foreach (var i in items)
                {
                    var icon = StartupService.LoadIcon(i);
                    if (icon != null) dispatcher.InvokeAsync(() => i.Icon = icon);
                }
            });
        }
        finally
        {
            IsLoading = false;
            OnPropertyChanged(nameof(Summary));
        }
    }

    private void Raise()
    {
        foreach (var n in new[] { nameof(EnabledCount), nameof(Summary), nameof(CountPrograms), nameof(CountTasks), nameof(CountServices) })
            OnPropertyChanged(n);
        _main.UpdateBadges();
        Changed?.Invoke();
    }

    private async void OnItemChanged(object sender, PropertyChangedEventArgs e)
    {
        if (_suppress || e.PropertyName != nameof(StartupItem.IsEnabled)) return;
        var item = (StartupItem)sender;
        bool enabled = item.IsEnabled;
        try
        {
            await Task.Run(() => StartupService.SetEnabled(item, enabled));
            ActionJournal.Add("startup", enabled ? $"«{item.Title}» включена в автозагрузку" : $"«{item.Title}» убрана из автозагрузки",
                item.SourceText);
            _main.ShowToast(enabled ? $"«{item.Title}» будет запускаться с Windows" : $"«{item.Title}» убрана из автозагрузки",
                enabled ? "info" : "success");
        }
        catch (Exception)
        {
            _suppress = true;
            item.IsEnabled = !enabled;
            _suppress = false;
            _main.ShowToast("Не хватает прав — запустите «Простор» от имени администратора", "warning");
        }
        Raise();
    }

    private async Task ToggleDelayAsync(StartupItem item)
    {
        try
        {
            if (item.IsDelayed)
            {
                string task = item.DelayTaskPath, key = item.Key;
                await Task.Run(() => StartupService.UndoDelayed(key, task));
                item.DelayTaskPath = null;
                ActionJournal.Add("startup", $"«{item.Title}» снова запускается сразу", item.SourceText);
                _main.ShowToast($"«{item.Title}» будет запускаться сразу при входе", "info");
            }
            else
            {
                string task = await Task.Run(() => StartupService.MakeDelayed(item));
                _suppress = true;
                item.IsEnabled = true;
                _suppress = false;
                ActionJournal.Add("startup", $"«{item.Title}» запускается через минуту после входа", item.SourceText,
                    undo: UndoKind.DelayedStart, undoData: new[] { item.Key, task });
                _main.ShowToast($"«{item.Title}» будет запускаться через минуту после входа — рабочий стол появится быстрее");
            }
        }
        catch (Exception ex)
        {
            await _main.ShowError("Не удалось изменить запуск", ex.Message);
        }
        Raise();
    }
}
