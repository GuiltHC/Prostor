using System.Diagnostics;
using System.Windows;
using Prostor.Core;
using Prostor.Views;

namespace Prostor.Services;

/// <summary>Registers the scheduled cleanup in Task Scheduler. Nothing of «Простор» keeps running in between.</summary>
public static class AutoCleanScheduler
{
    public const string TaskPath = @"\Prostor\AutoClean";

    public static bool IsRegistered => TaskSchedulerHelper.Exists(TaskPath);

    /// <param name="mode">"off", "weekly" or "monthly".</param>
    public static void Apply(string mode)
    {
        if (mode is not ("weekly" or "monthly"))
        {
            TaskSchedulerHelper.Delete(TaskPath);
            return;
        }
        string xml = TaskSchedulerHelper.CalendarTaskXml(
            "Простор: автоматическая очистка временных файлов и кэшей. Программа запускается, чистит и сразу закрывается.",
            mode == "monthly", Environment.ProcessPath!, "--auto-clean");
        TaskSchedulerHelper.Register(TaskPath, xml);
    }

    /// <summary>The task starts a different copy of the program (it was moved).</summary>
    public static bool PointsElsewhere
    {
        get
        {
            try
            {
                string xml = TaskSchedulerHelper.ExportXml(TaskPath);
                return !xml.Contains(Environment.ProcessPath!, StringComparison.OrdinalIgnoreCase);
            }
            catch
            {
                return false;
            }
        }
    }
}

/// <summary>Headless run started by Task Scheduler ("--auto-clean"): cleans, shows a short notice and exits.</summary>
public static class AutoCleanRunner
{
    public static async Task RunAsync()
    {
        long freed = 0;
        int categories = 0;
        try
        {
            var settings = AppSettings.Current;
            var items = await Task.Run(CleanupService.BuildItems);
            var selected = items.Where(i => !i.IsLocked && !i.IsRecycleBin &&
                                            (settings.CleanupChecks.TryGetValue(i.Id, out bool c) ? c : i.IsChecked)).ToList();
            foreach (var item in selected)
            {
                var r = await Task.Run(() => CleanupService.Clean(item, CancellationToken.None));
                if (r.FreedBytes > 0) categories++;
                freed += r.FreedBytes;
            }
            settings.LastAutoClean = DateTime.Now;
            settings.LastAutoCleanFreed = freed;
            settings.Save();
            ActionJournal.Add("autoclean", "Автоматическая очистка",
                $"{Fmt.Count(categories, "категория", "категории", "категорий")} · по расписанию", freed);
            DiskHistory.Record();
        }
        catch (Exception ex)
        {
            App.Log(ex);
        }

        string text = freed > 0 ? $"Освобождено {Fmt.Size(freed)}" : "Мусора не нашлось — всё чисто";
        var notice = new NotificationWindow("Автоочистка «Простор»", text);
        notice.Closed += (_, _) => Application.Current.Shutdown();
        notice.Show();
    }
}
