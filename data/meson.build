# The D-Bus introspection XML is generated at build time in top-level meson.build.
if get_option('dbus')
  python = import('python').find_installation()
  # Joining with `/` rewrites a backslash in either option to `/`, silently
  # changing an unusual but valid bindir before configure-service.py can encode
  # it for the service file. Join explicitly and keep the absolute-bindir rule:
  # an absolute bindir ignores the prefix.
  bindir = get_option('bindir')
  full_bindir = bindir.startswith('/') ? bindir : get_option('prefix') + '/' + bindir
  configure_file(
    input: 'org.verbisage.Dictionary.service.in',
    output: 'org.verbisage.Dictionary.service',
    command: [
      python, files('configure-service.py'), '@INPUT@', '@OUTPUT@',
      full_bindir,
    ],
    install: true,
    install_dir: get_option('datadir') / 'dbus-1' / 'services',
  )
endif
