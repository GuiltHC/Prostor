using Prostor.Core;

namespace Prostor.Services;

/// <summary>Rolls back journal entries.</summary>
public static class UndoService
{
    public static string Describe(UndoKind kind) => kind switch
    {
        UndoKind.RegImport => "Восстановить запись в реестре",
        UndoKind.RecycleRestore => "Вернуть из корзины",
        UndoKind.TaskRestore => "Восстановить задачу",
        UndoKind.Decompress => "Распаковать",
        UndoKind.MoveBack => "Вернуть на место",
        UndoKind.Hibernation => "Включить гибернацию",
        UndoKind.DelayedStart => "Вернуть обычный запуск",
        _ => null
    };

    /// <summary>Returns null on success, otherwise a message for the user.</summary>
    public static async Task<string> UndoAsync(JournalEntry e, IProgress<string> status = null)
    {
        try
        {
            switch (e.Undo)
            {
                case UndoKind.RegImport:
                {
                    int ok = 0;
                    foreach (var file in e.UndoData)
                    {
                        if (!File.Exists(file)) continue;
                        var r = await Cmd.RunAsync(Cmd.SystemTool("reg.exe"), $"import {Cmd.Quote(file)}");
                        if (r.Ok) ok++;
                    }
                    if (ok == 0)
                        return ShellHelper.IsAdmin
                            ? "Не удалось импортировать резервную копию реестра."
                            : "Не удалось импортировать резервную копию. Для записей «для всех пользователей» нужны права администратора.";
                    return ok == e.UndoData.Count ? null : $"Восстановлено {ok} из {e.UndoData.Count}.";
                }
                case UndoKind.RecycleRestore:
                {
                    int ok = 0;
                    foreach (var path in e.UndoData)
                    {
                        status?.Report(path);
                        if (await Task.Run(() => RecycleBin.Restore(path))) ok++;
                    }
                    if (ok == 0) return "В корзине не нашлось этих объектов — возможно, её уже очистили.";
                    return ok == e.UndoData.Count ? null : $"Возвращено {ok} из {e.UndoData.Count} — остальных уже нет в корзине.";
                }
                case UndoKind.TaskRestore:
                {
                    foreach (var pair in e.UndoData)
                    {
                        var parts = pair.Split('|', 2);
                        if (parts.Length == 2 && File.Exists(parts[1]))
                            await Task.Run(() => TaskSchedulerHelper.ImportXml(parts[0], File.ReadAllText(parts[1])));
                    }
                    return null;
                }
                case UndoKind.Decompress:
                {
                    foreach (var folder in e.UndoData)
                    {
                        status?.Report(folder);
                        await Compression.DecompressAsync(folder, null, CancellationToken.None);
                    }
                    return null;
                }
                case UndoKind.MoveBack:
                {
                    if (e.UndoData.Count < 2) return "Нет данных для возврата.";
                    await FolderMover.MoveBackAsync(e.UndoData[0], e.UndoData[1],
                        new Progress<double>(p => status?.Report(Fmt.Percent(p))), CancellationToken.None);
                    return null;
                }
                case UndoKind.Hibernation:
                {
                    var r = await Cmd.RunAsync(Cmd.SystemTool("powercfg.exe"), "/hibernate on");
                    return r.Ok ? null : "Не удалось включить гибернацию: нужны права администратора.";
                }
                case UndoKind.DelayedStart:
                {
                    if (e.UndoData.Count < 2) return "Нет данных для возврата.";
                    await Task.Run(() => StartupService.UndoDelayed(e.UndoData[0], e.UndoData[1]));
                    return null;
                }
                default:
                    return "Это действие нельзя отменить.";
            }
        }
        catch (Exception ex)
        {
            return ex.Message;
        }
    }
}
