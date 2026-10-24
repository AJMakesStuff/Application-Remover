import threading
from pathlib import Path
import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Adw, Gdk, Gio, GLib, GObject, Gtk, Pango
from . import backend


class Item(GObject.Object):
    def __init__(self, package):
        super().__init__()
        self.package = package
        self.search = ' '.join((package.name, package.description, package.ref)).casefold()


class Window(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title='Application Remover', default_width=980, default_height=720)
        self.packages = []
        self.busy = False
        self.progress_dialog = None
        self.set_size_request(560, 400)
        self.icon_theme = Gtk.IconTheme.get_for_display(self.get_display())
        layout = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.set_content(layout)
        header = Adw.HeaderBar()
        header.set_title_widget(Adw.WindowTitle(title='Application Remover', subtitle='Installed apps and system packages'))
        self.refresh_button = Gtk.Button(icon_name='view-refresh-symbolic', tooltip_text='Refresh installed packages')
        self.refresh_button.connect('clicked', lambda *_: self.refresh())
        header.pack_end(self.refresh_button)
        layout.append(header)
        toolbar = Gtk.Box(spacing=12, margin_start=18, margin_end=18, margin_top=12, margin_bottom=12)
        self.search = Gtk.SearchEntry(placeholder_text='Search names and descriptions…', hexpand=True)
        self.search.connect('search-changed', lambda *_: self.refilter())
        toolbar.append(self.search)
        self.source = Gtk.DropDown.new_from_strings(['All sources', 'Debian', 'Snap', 'Flatpak'])
        self.source.connect('notify::selected', lambda *_: self.refilter())
        toolbar.append(self.source)
        self.kind = Gtk.DropDown.new_from_strings(['All types', 'Applications', 'Linux stuff'])
        self.kind.connect('notify::selected', lambda *_: self.refilter())
        toolbar.append(self.kind)
        layout.append(toolbar)
        self.status = Gtk.Label(xalign=0, margin_start=18, margin_end=18, margin_bottom=10)
        self.status.add_css_class('dim-label')
        layout.append(self.status)
        self.store = Gio.ListStore.new(Item)
        self.filter = Gtk.CustomFilter.new(self.matches)
        self.filtered = Gtk.FilterListModel.new(self.store, self.filter)
        self.selection = Gtk.SingleSelection.new(self.filtered)
        self.selection.set_autoselect(False)
        self.selection.set_can_unselect(True)
        self.selection.connect('notify::selected-item', self.selected)
        factory = Gtk.SignalListItemFactory()
        factory.connect('setup', self.setup_row)
        factory.connect('bind', self.bind_row)
        self.list = Gtk.ListView(model=self.selection, factory=factory)
        self.list.add_css_class('navigation-sidebar')
        scroll = Gtk.ScrolledWindow(vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER)
        scroll.set_child(self.list)
        layout.append(scroll)
        layout.append(Gtk.Separator())
        footer = Gtk.Box(spacing=18, margin_start=18, margin_end=18, margin_top=16, margin_bottom=16)
        self.detail = Gtk.Label(label='Select a package to inspect and uninstall it.', xalign=0, hexpand=True,
                                wrap=True, selectable=True)
        footer.append(self.detail)
        self.remove_button = Gtk.Button(label='Uninstall…', sensitive=False, valign=Gtk.Align.CENTER)
        self.remove_button.add_css_class('destructive-action')
        self.remove_button.connect('clicked', self.prepare_remove)
        footer.append(self.remove_button)
        layout.append(footer)
        self.connect('close-request', lambda *_: self.busy)
        self.refresh()

    def setup_row(self, factory, item):
        row = Gtk.Box(spacing=12, margin_start=10, margin_end=10, margin_top=9, margin_bottom=9)
        icon = Gtk.Image(icon_name='application-x-executable-symbolic', pixel_size=36)
        row.append(icon)
        text = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3, hexpand=True)
        name = Gtk.Label(xalign=0, ellipsize=Pango.EllipsizeMode.END)
        name.add_css_class('heading')
        description = Gtk.Label(xalign=0, ellipsize=Pango.EllipsizeMode.END)
        description.add_css_class('dim-label')
        text.append(name)
        text.append(description)
        row.append(text)
        source = Gtk.Label()
        source.add_css_class('dim-label')
        row.append(source)
        item.set_child(row)
        item.widgets = (icon, name, description, source)
        gesture = Gtk.GestureClick()
        gesture.set_button(Gdk.BUTTON_SECONDARY)
        gesture.connect('pressed', self.row_context_menu, item, row)
        row.add_controller(gesture)

    def bind_row(self, factory, item):
        p = item.get_item().package
        icon, name, description, source = item.widgets
        fallback_icon = ('changes-prevent-symbolic' if p.protected else
                         'application-x-executable-symbolic' if p.application else 'package-x-generic-symbolic')
        icon.set_from_icon_name(fallback_icon)
        if p.icon:
            icon_path = Path(p.icon)
            if icon_path.is_file():
                icon.set_from_file(str(icon_path))
            elif not icon_path.is_absolute() and self.icon_theme.has_icon(p.icon):
                icon.set_from_icon_name(p.icon)
        name.set_text(p.name)
        description.set_text(p.description or p.ref or p.version)
        source.set_text(p.source + (f' · {p.scope}' if p.scope else ''))

    def row_context_menu(self, gesture, n_press, x, y, list_item, row):
        package_item = list_item.get_item()
        if package_item is None:
            return
        self.selection.set_selected(list_item.get_position())

        popover = Gtk.Popover()
        menu = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2,
                       margin_start=6, margin_end=6, margin_top=6, margin_bottom=6)

        def add_menu_item(label, callback):
            button = Gtk.Button(label=label, halign=Gtk.Align.FILL, hexpand=True)
            button.add_css_class('flat')

            def clicked(*_):
                popover.popdown()
                def activate():
                    callback()
                    return False
                GLib.idle_add(activate)

            button.connect('clicked', clicked)
            menu.append(button)

        add_menu_item('Show dependencies and details…', self.show_package_details)
        if not package_item.package.protected:
            add_menu_item('Uninstall…', self.prepare_remove)
        popover.set_child(menu)
        popover.set_parent(row)
        point = Gdk.Rectangle()
        point.x, point.y, point.width, point.height = int(x), int(y), 1, 1
        popover.set_pointing_to(point)
        popover.connect('closed', lambda closed: closed.unparent() if closed.get_parent() else None)
        popover.popup()

    def matches(self, item):
        source = ['All sources', 'Debian', 'Snap', 'Flatpak'][self.source.get_selected()]
        kind = self.kind.get_selected()
        matches_kind = (kind == 0 or (kind == 1 and item.package.application)
                        or (kind == 2 and not item.package.application))
        return (matches_kind and (source == 'All sources' or source == item.package.source)
                and all(word in item.search for word in self.search.get_text().casefold().split()))

    def refilter(self):
        if not hasattr(self, 'filter'):
            return
        self.filter.changed(Gtk.FilterChange.DIFFERENT)
        self.update_count()

    def update_count(self):
        self.status.set_text(f'{self.filtered.get_n_items():,} shown · {len(self.packages):,} installed'
                             if self.packages else 'No packages found')

    def selected(self, *_):
        item = self.selection.get_selected_item()
        self.remove_button.set_sensitive(bool(item) and not self.busy and not item.package.protected)
        if item:
            p = item.package
            size = f' · {GLib.format_size(p.size)} installed' if p.size else ''
            protected = '\nEssential / protected system package' if p.protected else ''
            installed = p.install_date or 'Unknown'
            self.detail.set_text(f'{p.name}\n{p.version} · {p.source}{size}\nInstalled: {installed}{protected}')
        else:
            self.detail.set_text('Select a package to inspect and uninstall it.')

    def show_package_details(self):
        item = self.selection.get_selected_item()
        if self.busy:
            self.message('Please wait', 'The package list is being updated or another operation is running.')
            return
        if item is None:
            self.message('Select a package', 'Choose a package row before opening its details.')
            return
        package = item.package
        self.work('Reading package metadata and dependencies…',
                  lambda: backend.package_details(package),
                  lambda details: self.show_details(package, details),
                  progress_heading='Loading package details')

    def show_details(self, package, details):
        dialog = Adw.Dialog(content_width=700)
        dialog.set_title(f'{package.name} details')
        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=14,
                          margin_start=20, margin_end=20, margin_top=20, margin_bottom=20)
        description = Gtk.Label(label='Package metadata, dependencies, and related information.',
                                xalign=0, wrap=True)
        description.add_css_class('dim-label')
        content.append(description)
        scroll = Gtk.ScrolledWindow(min_content_width=640, min_content_height=300,
                                    max_content_height=480, vexpand=True,
                                    propagate_natural_height=True)
        text = Gtk.Label(label=details, xalign=0, yalign=0, wrap=True, selectable=True)
        text.add_css_class('monospace')
        scroll.set_child(text)
        content.append(scroll)
        close = Gtk.Button(label='Close', halign=Gtk.Align.END)
        close.connect('clicked', lambda *_: dialog.force_close())
        content.append(close)
        dialog.set_child(content)
        dialog.present(self)

    def show_progress(self, heading, message):
        dialog = Adw.Dialog(can_close=False, content_width=380)
        dialog.set_title(heading)
        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16,
                          margin_start=28, margin_end=28, margin_top=28, margin_bottom=28)
        spinner = Gtk.Spinner(spinning=True, width_request=32, height_request=32,
                              halign=Gtk.Align.CENTER)
        content.append(spinner)
        title = Gtk.Label(label=heading, wrap=True, justify=Gtk.Justification.CENTER)
        title.add_css_class('title-2')
        content.append(title)
        description = Gtk.Label(label=message, wrap=True, justify=Gtk.Justification.CENTER)
        description.add_css_class('dim-label')
        content.append(description)
        dialog.set_child(content)
        self.progress_dialog = dialog
        dialog.present(self)

    def work(self, message, function, done, *, progress_heading=None):
        self.busy = True
        self.refresh_button.set_sensitive(False)
        self.remove_button.set_sensitive(False)
        if progress_heading:
            self.show_progress(progress_heading, message)
        else:
            self.status.set_text(message)
        def worker():
            try:
                value, error = function(), None
            except Exception as exc:
                value, error = None, str(exc)
            GLib.idle_add(finish, value, error)
        def finish(value, error):
            if self.progress_dialog is not None:
                self.progress_dialog.force_close()
                self.progress_dialog = None
            self.busy = False
            self.refresh_button.set_sensitive(True)
            self.update_count()
            self.selected()
            if error:
                self.message('Operation failed', error)
            else:
                done(value)
            return False
        threading.Thread(target=worker, daemon=True).start()

    def message(self, heading, body):
        dialog = Adw.MessageDialog(transient_for=self, heading=heading, body=body)
        dialog.add_response('close', 'Close')
        dialog.present()

    def refresh(self, *, modal=False):
        if self.busy:
            return
        def loaded(result):
            self.packages, warnings = result
            self.store.splice(0, self.store.get_n_items(), [Item(p) for p in self.packages])
            self.update_count()
            if warnings:
                self.message('Some sources could not be loaded', '\n\n'.join(warnings))
        self.work('Reading installed packages…', backend.inventory, loaded,
                  progress_heading='Updating package list' if modal else None)

    def prepare_remove(self, *_):
        item = self.selection.get_selected_item()
        if self.busy or not item:
            return
        package = item.package
        self.work('Checking removal dependencies…', lambda: backend.removal_plan(package, self.packages),
                  lambda plan: self.confirm(package, plan), progress_heading='Preparing uninstall')

    def confirm(self, package, plan):
        body = ('Review every package below. Removing system components can affect your desktop. '
                'For Debian packages, automatically installed dependencies that become unused are also removed. '
                'Shared dependencies and manually installed libraries are kept. '
                'Personal files and Debian configuration files are kept. '
                'Administrator authentication may be required.')
        dialog = Adw.MessageDialog(transient_for=self, heading=f'Uninstall {package.name}?', body=body)
        label = Gtk.Label(label='\n'.join(plan.names), xalign=0, yalign=0, selectable=True, margin_start=12, margin_end=12,
                          margin_top=12, margin_bottom=12)
        scroll = Gtk.ScrolledWindow(min_content_height=100, max_content_height=260,
                                   propagate_natural_height=True)
        scroll.set_child(label)
        dialog.set_extra_child(scroll)
        dialog.add_response('cancel', 'Cancel')
        dialog.add_response('remove', f'Uninstall ({len(plan.names)})')
        dialog.set_response_appearance('remove', Adw.ResponseAppearance.DESTRUCTIVE)
        dialog.set_default_response('cancel')
        dialog.set_close_response('cancel')
        def response(_, choice):
            if choice == 'remove':
                self.work('Uninstalling… Complete the authentication prompt if shown.',
                          lambda: backend.remove(package, self.packages, plan), lambda _: self.refresh(modal=True),
                          progress_heading=f'Uninstalling {package.name}')
        dialog.connect('response', response)
        dialog.present()


def main():
    app = Adw.Application(application_id='io.github.applicationremover.App', flags=Gio.ApplicationFlags.DEFAULT_FLAGS)
    def activate(app):
        window = app.get_active_window() or Window(app)
        window.present()
    app.connect('activate', activate)
    app.run(None)
