project('verbisage', 'rust',
  version: '0.1.0',
  license: 'MIT OR Apache-2.0',
  meson_version: '>=1.1.0',
)

# ── Tools ────────────────────────────────────────────────────────────────────
cargo = find_program('cargo', required: true)

# ── Feature flags ────────────────────────────────────────────────────────────
features = []
foreach f : ['sqlite', 'hunspell', 'dbus']
  if get_option(f)
    features += f
  endif
endforeach

cargo_features = features.length() > 0 ? '--features=' + ','.join(features) : ''

# ── Build stamp (always stale — Cargo handles its own incremental rebuilds) ──
custom_target('cargo-build',
  output: 'cargo-build.stamp',
  build_always_stale: true,
  build_by_default: true,
  console: true,
  command: [
    cargo, 'build', '--release', cargo_features,
    '--manifest-path', '@CURRENT_SOURCE_DIR@/Cargo.toml',
    '&&', 'touch', '@OUTPUT@',
  ],
)

# ── Paths shared by install scripts ──────────────────────────────────────────
_release_dir = meson.project_source_root() / 'target' / 'release'

# ── Binaries ─────────────────────────────────────────────────────────────────
meson.add_install_script('sh', '-c',
  'install -D -m 755 "$1/verbisage" "${MESON_INSTALL_DESTDIR_PREFIX}/$2"',
  'sh',
  _release_dir,
  get_option('bindir') / 'verbisage',
)
meson.add_install_script('sh', '-c',
  'install -D -m 755 "$1/verbisaged" "${MESON_INSTALL_DESTDIR_PREFIX}/$2"',
  'sh',
  _release_dir,
  get_option('bindir') / 'verbisaged',
)

# ── Shared library ───────────────────────────────────────────────────────────
meson.add_install_script('sh', '-c',
  'install -D -m 644 "$1/libverbisage.so" "${MESON_INSTALL_DESTDIR_PREFIX}/$2"',
  'sh',
  _release_dir,
  get_option('libdir') / 'libverbisage.so',
)

# ── Data files ───────────────────────────────────────────────────────────────
subdir('data')
