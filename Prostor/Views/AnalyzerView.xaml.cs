using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using Prostor.Services;
using Prostor.ViewModels;

namespace Prostor.Views;

public partial class AnalyzerView : UserControl
{
    private AnalyzerViewModel Vm => DataContext as AnalyzerViewModel;

    public AnalyzerView()
    {
        InitializeComponent();

        Treemap.HoverChanged += hit => Vm?.SetHover(hit);
        Treemap.Clicked += hit => Vm?.SelectByHit(hit);
        Treemap.DoubleClicked += hit => Vm?.DrillByHit(hit);
        Treemap.RightClicked += hit =>
        {
            var vm = Vm;
            if (vm == null) return;
            vm.SelectByHit(hit);
            var item = vm.ItemFromHit(hit);
            if (item == null) return;
            var menu = (ContextMenu)FindResource("NodeMenu");
            Treemap.Tag = vm;
            menu.PlacementTarget = Treemap;
            menu.Placement = PlacementMode.MousePoint;
            menu.DataContext = item;
            menu.IsOpen = true;
        };

        ChildList.SelectionChanged += (_, _) =>
        {
            if (ChildList.SelectedItem != null) ChildList.ScrollIntoView(ChildList.SelectedItem);
        };
        DataContextChanged += (_, _) =>
        {
            if (Vm != null)
                Vm.PropertyChanged += (_, e) =>
                {
                    if (e.PropertyName == nameof(AnalyzerViewModel.Current))
                        Dispatcher.BeginInvoke(() => CrumbScroll.ScrollToRightEnd());
                };
        };
    }

    private void ChildList_MouseDoubleClick(object sender, MouseButtonEventArgs e)
    {
        if (e.OriginalSource is DependencyObject d && ItemsControl.ContainerFromElement(ChildList, d) is ListBoxItem { DataContext: NodeItem item })
            Vm?.OpenItemCommand.Execute(item);
    }

    private void ChildList_KeyDown(object sender, KeyEventArgs e)
    {
        if (Vm == null) return;
        if (e.Key == Key.Enter && Vm.SelectedChild != null)
        {
            Vm.OpenItemCommand.Execute(Vm.SelectedChild);
            e.Handled = true;
        }
        else if (e.Key == Key.Back)
        {
            Vm.UpCommand.Execute(null);
            e.Handled = true;
        }
        else if (e.Key == Key.Delete && Vm.SelectedChild is { CanAct: true } item)
        {
            Vm.DeleteCommand.Execute(item);
            e.Handled = true;
        }
    }

    private void LargeList_MouseDoubleClick(object sender, MouseButtonEventArgs e)
    {
        if (e.OriginalSource is DependencyObject d && ItemsControl.ContainerFromElement(LargeList, d) is ListBoxItem { DataContext: LargeFile f })
            Vm?.OpenLargeFileFolderCommand.Execute(f);
    }

    private void ChangeList_MouseDoubleClick(object sender, MouseButtonEventArgs e)
    {
        if (e.OriginalSource is DependencyObject d && ItemsControl.ContainerFromElement(ChangeList, d) is ListBoxItem { DataContext: FolderChange c })
            Vm?.OpenChangeCommand.Execute(c);
    }
}
