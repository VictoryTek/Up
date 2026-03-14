project('up',
  version: '0.1.0',
  license: 'GPL-3.0-or-later',
)

cargo = find_program('cargo')

builddir = meson.current_build_dir()
srcdir = meson.current_source_dir()

prefix = get_option('prefix')
bindir = join_paths(prefix, get_option('bindir'))
datadir = join_paths(prefix, get_option('datadir'))

if get_option('buildtype') == 'release'
  rust_target = 'release'
  cargo_args = ['build', '--release', '--manifest-path', join_paths(srcdir, 'Cargo.toml')]
else
  rust_target = 'debug'
  cargo_args = ['build', '--manifest-path', join_paths(srcdir, 'Cargo.toml')]
endif

cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [cargo] + cargo_args,
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

install_data('data/io.github.up.svg',
  install_dir: join_paths(datadir, 'icons', 'hicolor', 'scalable', 'apps'),
)
