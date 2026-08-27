project(
  'splitdesk-weston-input',
  'c',
  version: '0.1.0',
  default_options: ['c_std=c11', 'warning_level=3'],
)

weston_dep = dependency('libweston-16', required: false)
foreach version : ['15', '14', '13']
  if not weston_dep.found()
    weston_dep = dependency('libweston-' + version, required: false)
  endif
endforeach
if not weston_dep.found()
  error('SplitDesk requires libweston development headers version 13 through 16')
endif

wayland_server_dep = dependency('wayland-server')

shared_module(
  'splitdesk-input',
  'splitdesk-input.c',
  name_prefix: '',
  dependencies: [weston_dep, wayland_server_dep],
  install: true,
  install_dir: join_paths(get_option('libdir'), 'splitdesk'),
)
