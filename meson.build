project('up',
  version: run_command(
    'grep', '-m', '1', '^version', 'Cargo.toml',
    check: true,
  ).stdout().strip().split('"')[1],
  license: 'GPL-3.0-or-later',
)

cargo = find_program('cargo')
fs = import('fs')
gnome = import('gnome')
i18n = import('i18n')

builddir = meson.current_build_dir()
srcdir = meson.current_source_dir()

prefix = get_option('prefix')
bindir = join_paths(prefix, get_option('bindir'))
datadir = join_paths(prefix, get_option('datadir'))
localedir = join_paths(prefix, get_option('localedir'))

if get_option('buildtype') == 'release'
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [
    'sh', '-c',
    'LOCALEDIR=' + localedir + ' ' +
    cargo.full_path() + ' build ' +
    (rust_target == 'release' ? '--release ' : '') +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' --target-dir ' + meson.build_root() / 'cargo-target' +
    ' && cp ' + meson.build_root() / 'cargo-target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  depend_files: files('Cargo.toml', 'Cargo.lock'),
  console: true,
  install: true,
  install_dir: bindir,
)

desktop_file = i18n.merge_file(
  input: 'data/io.github.up.desktop.in',
  output: 'io.github.up.desktop',
  type: 'desktop',
  po_dir: 'po',
  install: true,
  install_dir: join_paths(datadir, 'applications'),
)

install_data('data/io.github.up.metainfo.xml',
  install_dir: join_paths(datadir, 'metainfo'),
)

install_data('data/io.github.up.policy',
  install_dir: join_paths(datadir, 'polkit-1', 'actions'),
)

foreach size : ['256x256', '128x128', '48x48']
  png = 'data/icons/hicolor' / size / 'apps/io.github.up.png'
  if fs.exists(png)
    install_data(png,
      install_dir: join_paths(datadir, 'icons', 'hicolor', size, 'apps'),
    )
  endif
endforeach

subdir('po')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
