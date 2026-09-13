# The D-Bus introspection XML is generated at build time in top-level meson.build.
if get_option('dbus')
  python = import('python').find_installation()
  configure_file(
    input: 'org.verbisage.Dictionary.service.in',
    output: 'org.verbisage.Dictionary.service',
    command: [
      python, files('configure-service.py'), '@INPUT@', '@OUTPUT@',
      get_option('prefix') / get_option('bindir'),
    ],
    install: true,
    install_dir: get_option('datadir') / 'dbus-1' / 'services',
  )
endif
