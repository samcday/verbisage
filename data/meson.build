# When the dbus feature is disabled no introspection XML is generated from
# code, so we install the checked-in snapshot instead.
if not get_option('dbus')
  install_data(
    'org.verbisage.Dictionary.xml',
    install_dir: dbus_interfaces_dir,
  )
endif
