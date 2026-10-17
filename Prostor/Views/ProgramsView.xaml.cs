using System.Windows.Controls;
using System.Windows.Input;
using Prostor.ViewModels;

namespace Prostor.Views;

public partial class ProgramsView : UserControl
{
    public ProgramsView()
    {
        InitializeComponent();
        DataContextChanged += (_, _) =>
        {
            if (DataContext is ProgramsViewModel vm)
                vm.PropertyChanged += (_, e) =>
                {
                    if (e.PropertyName == nameof(ProgramsViewModel.Selected) && vm.Selected != null)
                        Dispatcher.BeginInvoke(() => List.ScrollIntoView(vm.Selected));
                };
        };
    }

    private void List_MouseDoubleClick(object sender, MouseButtonEventArgs e)
    {
        if (DataContext is ProgramsViewModel { Selected: { HasLocation: true } app } vm)
            vm.OpenFolderCommand.Execute(app);
    }
}
