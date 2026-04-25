project('up',
  version: '1.0.2',
  license: 'GPL-3.0-or-later',
)

cargo = find_program('cargo')
fs = import('fs')
gnome = import('gnome')

builddir = meson.current_build_dir()
srcdir = meson.current_source_dir()

prefix = get_option('prefix')
bindir = join_paths(prefix, get_option('bindir'))
datadir = join_paths(prefix, get_option('datadir'))

if get_option('buildtype') == 'release'
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [
    'sh', '-c',
    cargo.full_path() + ' build ' +
    (rust_target == 'release' ? '--release ' : '') +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' && cp ' + srcdir / 'target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  build_always_stale: true,
  console: true,
  install: true,
  install_dir: bindir,
)

install_data('data/io.github.up.desktop',
  install_dir: join_paths(datadir, 'applications'),
)

install_data('data/io.github.up.metainfo.xml',
  install_dir: join_paths(datadir, 'metainfo'),
)

foreach size : ['256x256', '128x128', '48x48']
  png = 'data/icons/hicolor' / size / 'apps/io.github.up.png'
  if fs.exists(png)
    install_data(png,
      install_dir: join_paths(datadir, 'icons', 'hicolor', size, 'apps'),
    )
  endif
endforeach

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
