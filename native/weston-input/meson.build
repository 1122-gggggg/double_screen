project(
  'splitdesk-weston-input',
  'c',
  version: '0.2.0',
  default_options: ['c_std=c11', 'warning_level=3'],
)

weston_major = '16'
weston_dep = dependency('libweston-' + weston_major, required: false)
foreach version : ['15', '14', '13']
  if not weston_dep.found()
    weston_major = version
    weston_dep = dependency('libweston-' + version, required: false)
  endif
endforeach
if not weston_dep.found()
  error('SplitDesk requires libweston development headers version 13 through 16')
endif
weston_dep = weston_dep.as_system()
add_project_arguments('-DSPLITDESK_WESTON_MAJOR=' + weston_major, language: 'c')

wayland_server_dep = dependency('wayland-server').as_system()

shared_module(
  'splitdesk-input',
  'splitdesk-input.c',
  name_prefix: '',
  dependencies: [weston_dep, wayland_server_dep],
  install: true,
  install_dir: join_paths(get_option('libdir'), 'splitdesk'),
)
