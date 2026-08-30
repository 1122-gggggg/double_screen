#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <linux/input-event-codes.h>
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#include <libweston/libweston.h>
#include <libweston/weston-log.h>
#include <wayland-server-core.h>

#define SPLITDESK_INPUT_MAGIC 0x4e494453u /* "SDIN" little-endian */
#define SPLITDESK_INPUT_VERSION 1u
#define SPLITDESK_PACKET_SIZE 40u
#define SPLITDESK_RX_CAPACITY (SPLITDESK_PACKET_SIZE * 32u)
#define SPLITDESK_KEY_CAPACITY 768u

enum splitdesk_input_kind {
	SPLITDESK_POINTER_MOTION = 1,
	SPLITDESK_POINTER_BUTTON = 2,
	SPLITDESK_KEY = 3,
	SPLITDESK_SCROLL = 4,
};

/* These input-backend entry points are exported by libweston but are not part
 * of its installed public header. The module is therefore built against, and
 * must be rebuilt for, the libweston major installed on the host. */
void weston_seat_init(struct weston_seat *, struct weston_compositor *, const char *);
void weston_seat_release(struct weston_seat *);
void weston_seat_init_pointer(struct weston_seat *);
int weston_seat_init_keyboard(struct weston_seat *, struct xkb_keymap *);
#if SPLITDESK_WESTON_MAJOR >= 16
void notify_motion(const struct weston_pointer_motion_event *);
void notify_button(const struct weston_pointer_button_event *);
void notify_axis(const struct weston_pointer_axis_event *);
void notify_key(const struct weston_key_event *);
#else
void notify_motion(struct weston_seat *, const struct timespec *,
		   struct weston_pointer_motion_event *);
void notify_button(struct weston_seat *, const struct timespec *, int32_t,
		   enum wl_pointer_button_state);
void notify_axis(struct weston_seat *, const struct timespec *,
		 struct weston_pointer_axis_event *);
void notify_key(struct weston_seat *, const struct timespec *, uint32_t,
		enum wl_keyboard_key_state, enum weston_key_state_update);
#endif
void notify_axis_source(struct weston_seat *, uint32_t);
void notify_pointer_frame(struct weston_seat *);

struct splitdesk_input {
	struct weston_compositor *compositor;
	struct weston_seat seat;
	struct wl_listener destroy_listener;
	struct wl_event_source *listen_source;
	struct wl_event_source *client_source;
	int listen_fd;
	int client_fd;
	char socket_path[sizeof(((struct sockaddr_un *)0)->sun_path)];
	uint8_t rx[SPLITDESK_RX_CAPACITY];
	size_t rx_len;
	bool keys_down[SPLITDESK_KEY_CAPACITY];
	bool buttons_down[8];
};

static uint16_t
read_u16(const uint8_t *p)
{
	return (uint16_t)p[0] | ((uint16_t)p[1] << 8);
}

static uint32_t
read_u32(const uint8_t *p)
{
	return (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
	       ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static uint64_t
read_u64(const uint8_t *p)
{
	return (uint64_t)read_u32(p) | ((uint64_t)read_u32(p + 4) << 32);
}

static double
read_f64(const uint8_t *p)
{
	uint64_t bits = read_u64(p);
	double value;
	memcpy(&value, &bits, sizeof(value));
	return value;
}

static uint32_t
vk_to_evdev(uint32_t vk)
{
	static const uint16_t letters[] = {
		KEY_A, KEY_B, KEY_C, KEY_D, KEY_E, KEY_F, KEY_G,
		KEY_H, KEY_I, KEY_J, KEY_K, KEY_L, KEY_M, KEY_N,
		KEY_O, KEY_P, KEY_Q, KEY_R, KEY_S, KEY_T, KEY_U,
		KEY_V, KEY_W, KEY_X, KEY_Y, KEY_Z,
	};
	static const uint16_t digits[] = {
		KEY_0, KEY_1, KEY_2, KEY_3, KEY_4,
		KEY_5, KEY_6, KEY_7, KEY_8, KEY_9,
	};
	static const uint16_t keypad[] = {
		KEY_KP0, KEY_KP1, KEY_KP2, KEY_KP3, KEY_KP4,
		KEY_KP5, KEY_KP6, KEY_KP7, KEY_KP8, KEY_KP9,
	};

	if (vk >= 0x41 && vk <= 0x5a)
		return letters[vk - 0x41];
	if (vk >= 0x30 && vk <= 0x39)
		return digits[vk - 0x30];
	if (vk >= 0x60 && vk <= 0x69)
		return keypad[vk - 0x60];
	if (vk >= 0x70 && vk <= 0x79)
		return KEY_F1 + (vk - 0x70);
	if (vk == 0x7a) return KEY_F11;
	if (vk == 0x7b) return KEY_F12;
	if (vk == 0x7c) return KEY_F13;
	if (vk == 0x7d) return KEY_F14;
	if (vk == 0x7e) return KEY_F15;

	switch (vk) {
	case 0x08: return KEY_BACKSPACE;
	case 0x09: return KEY_TAB;
	case 0x0d: return KEY_ENTER;
	case 0x13: return KEY_PAUSE;
	case 0x14: return KEY_CAPSLOCK;
	case 0x1b: return KEY_ESC;
	case 0x20: return KEY_SPACE;
	case 0x21: return KEY_PAGEUP;
	case 0x22: return KEY_PAGEDOWN;
	case 0x23: return KEY_END;
	case 0x24: return KEY_HOME;
	case 0x25: return KEY_LEFT;
	case 0x26: return KEY_UP;
	case 0x27: return KEY_RIGHT;
	case 0x28: return KEY_DOWN;
	case 0x2d: return KEY_INSERT;
	case 0x2e: return KEY_DELETE;
	case 0x5b: return KEY_LEFTMETA;
	case 0x5c: return KEY_RIGHTMETA;
	case 0x5d: return KEY_MENU;
	case 0x6a: return KEY_KPASTERISK;
	case 0x6b: return KEY_KPPLUS;
	case 0x6d: return KEY_KPMINUS;
	case 0x6e: return KEY_KPDOT;
	case 0x6f: return KEY_KPSLASH;
	case 0x90: return KEY_NUMLOCK;
	case 0x91: return KEY_SCROLLLOCK;
	case 0xa0: return KEY_LEFTSHIFT;
	case 0xa1: return KEY_RIGHTSHIFT;
	case 0xa2: return KEY_LEFTCTRL;
	case 0xa3: return KEY_RIGHTCTRL;
	case 0xa4: return KEY_LEFTALT;
	case 0xa5: return KEY_RIGHTALT;
	case 0xba: return KEY_SEMICOLON;
	case 0xbb: return KEY_EQUAL;
	case 0xbc: return KEY_COMMA;
	case 0xbd: return KEY_MINUS;
	case 0xbe: return KEY_DOT;
	case 0xbf: return KEY_SLASH;
	case 0xc0: return KEY_GRAVE;
	case 0xdb: return KEY_LEFTBRACE;
	case 0xdc: return KEY_BACKSLASH;
	case 0xdd: return KEY_RIGHTBRACE;
	case 0xde: return KEY_APOSTROPHE;
	default: return 0;
	}
}

static uint32_t
button_to_evdev(uint32_t button)
{
	switch (button) {
	case 1: return BTN_LEFT;
	case 2: return BTN_RIGHT;
	case 3: return BTN_MIDDLE;
	case 4: return BTN_SIDE;
	case 5: return BTN_EXTRA;
	default: return 0;
	}
}

static void
splitdesk_notify_motion(struct splitdesk_input *input,
			const struct timespec *now, double x, double y)
{
#if SPLITDESK_WESTON_MAJOR >= 16
	struct timespec event_time = *now;
	struct weston_pointer_motion_event event;
	struct weston_coord_global abs = { .c = weston_coord(x, y) };
	weston_pointer_motion_event_init(&event, &event_time, &input->seat,
					 WESTON_POINTER_MOTION_ABS,
					 &abs, NULL, NULL);
	notify_motion(&event);
#else
	struct weston_pointer_motion_event event = {
		.mask = WESTON_POINTER_MOTION_ABS,
		.time = *now,
		.abs = {
			.c = {
				.x = x,
				.y = y,
			},
		},
	};
	notify_motion(&input->seat, now, &event);
#endif
}

static void
splitdesk_notify_button(struct splitdesk_input *input,
			const struct timespec *now, uint32_t button,
			enum wl_pointer_button_state state)
{
#if SPLITDESK_WESTON_MAJOR >= 16
	struct timespec event_time = *now;
	struct weston_pointer_button_event event;
	weston_pointer_button_event_init(&event, &event_time, &input->seat,
					 button, state);
	notify_button(&event);
#else
	notify_button(&input->seat, now, (int32_t)button, state);
#endif
}

static void
splitdesk_notify_axis(struct splitdesk_input *input,
		      const struct timespec *now, uint32_t axis,
		      double value, int32_t discrete)
{
#if SPLITDESK_WESTON_MAJOR >= 16
	struct timespec event_time = *now;
	struct weston_pointer_axis_event event;
	weston_pointer_axis_event_init(&event, &event_time, &input->seat,
				       axis, value, true, discrete);
	notify_axis(&event);
#else
	struct weston_pointer_axis_event event = {
		.axis = axis,
		.value = value,
		.has_discrete = true,
		.discrete = discrete,
	};
	notify_axis(&input->seat, now, &event);
#endif
}

static void
splitdesk_notify_key(struct splitdesk_input *input,
		     const struct timespec *now, uint32_t key,
		     enum wl_keyboard_key_state state)
{
#if SPLITDESK_WESTON_MAJOR >= 16
	struct timespec event_time = *now;
	struct weston_key_event event;
	weston_key_event_init(&event, &event_time, &input->seat, key, state,
			      STATE_UPDATE_AUTOMATIC);
	notify_key(&event);
#else
	notify_key(&input->seat, now, key, state, STATE_UPDATE_AUTOMATIC);
#endif
}

static void
release_all(struct splitdesk_input *input)
{
	struct timespec now;
	size_t i;

	clock_gettime(CLOCK_MONOTONIC, &now);
	for (i = 0; i < SPLITDESK_KEY_CAPACITY; i++) {
		if (input->keys_down[i]) {
			splitdesk_notify_key(input, &now, (uint32_t)i,
					     WL_KEYBOARD_KEY_STATE_RELEASED);
			input->keys_down[i] = false;
		}
	}
	for (i = 0; i < 8; i++) {
		if (input->buttons_down[i]) {
			uint32_t code = button_to_evdev((uint32_t)i);
			if (code)
				splitdesk_notify_button(input, &now, code,
						WL_POINTER_BUTTON_STATE_RELEASED);
			input->buttons_down[i] = false;
		}
	}
	notify_pointer_frame(&input->seat);
}

static void
close_client(struct splitdesk_input *input)
{
	if (input->client_source) {
		wl_event_source_remove(input->client_source);
		input->client_source = NULL;
	}
	if (input->client_fd >= 0) {
		close(input->client_fd);
		input->client_fd = -1;
	}
	input->rx_len = 0;
	release_all(input);
}

static bool
dispatch_packet(struct splitdesk_input *input, const uint8_t *packet)
{
	struct timespec now;
	uint16_t kind;

	if (read_u32(packet) != SPLITDESK_INPUT_MAGIC ||
	    read_u16(packet + 4) != SPLITDESK_INPUT_VERSION)
		return false;
	kind = read_u16(packet + 6);
	clock_gettime(CLOCK_MONOTONIC, &now);

	switch (kind) {
	case SPLITDESK_POINTER_MOTION: {
		double x = read_f64(packet + 16);
		double y = read_f64(packet + 24);
		if (!isfinite(x) || !isfinite(y))
			return false;
		splitdesk_notify_motion(input, &now, x, y);
		notify_pointer_frame(&input->seat);
		return true;
	}
	case SPLITDESK_POINTER_BUTTON: {
		uint32_t button = read_u32(packet + 32);
		uint32_t code = button_to_evdev(button);
		bool pressed = read_u32(packet + 36) != 0;
		if (!code || button >= 8)
			return false;
		splitdesk_notify_button(input, &now, code,
				pressed ? WL_POINTER_BUTTON_STATE_PRESSED :
					  WL_POINTER_BUTTON_STATE_RELEASED);
		input->buttons_down[button] = pressed;
		notify_pointer_frame(&input->seat);
		return true;
	}
	case SPLITDESK_KEY: {
		uint32_t code = vk_to_evdev(read_u32(packet + 32));
		bool pressed = read_u32(packet + 36) != 0;
		if (!code || code >= SPLITDESK_KEY_CAPACITY)
			return false;
		splitdesk_notify_key(input, &now, code,
			     pressed ? WL_KEYBOARD_KEY_STATE_PRESSED :
				       WL_KEYBOARD_KEY_STATE_RELEASED);
		input->keys_down[code] = pressed;
		return true;
	}
	case SPLITDESK_SCROLL: {
		double dx = read_f64(packet + 16);
		double dy = read_f64(packet + 24);
		if (!isfinite(dx) || !isfinite(dy))
			return false;
		notify_axis_source(&input->seat, WL_POINTER_AXIS_SOURCE_WHEEL);
		if (dy != 0.0)
			splitdesk_notify_axis(input, &now,
					      WL_POINTER_AXIS_VERTICAL_SCROLL,
					      -dy * 10.0, dy > 0.0 ? -1 : 1);
		if (dx != 0.0)
			splitdesk_notify_axis(input, &now,
					      WL_POINTER_AXIS_HORIZONTAL_SCROLL,
					      -dx * 10.0, dx > 0.0 ? -1 : 1);
		notify_pointer_frame(&input->seat);
		return true;
	}
	default:
		return false;
	}
}

static int
client_ready(int fd, uint32_t mask, void *data)
{
	struct splitdesk_input *input = data;
	ssize_t count;

	if (mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR)) {
		close_client(input);
		return 0;
	}
	count = recv(fd, input->rx + input->rx_len,
		     sizeof(input->rx) - input->rx_len, 0);
	if (count <= 0) {
		if (count < 0 && (errno == EAGAIN || errno == EINTR))
			return 0;
		close_client(input);
		return 0;
	}
	input->rx_len += (size_t)count;
	while (input->rx_len >= SPLITDESK_PACKET_SIZE) {
		if (!dispatch_packet(input, input->rx)) {
			close_client(input);
			return 0;
		}
		input->rx_len -= SPLITDESK_PACKET_SIZE;
		memmove(input->rx, input->rx + SPLITDESK_PACKET_SIZE,
			input->rx_len);
	}
	if (input->rx_len == sizeof(input->rx))
		close_client(input);
	return 0;
}

static int
listen_ready(int fd, uint32_t mask, void *data)
{
	struct splitdesk_input *input = data;
	struct ucred cred;
	socklen_t cred_len = sizeof(cred);
	int client;

	if (mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR))
		return 0;
	client = accept4(fd, NULL, NULL, SOCK_NONBLOCK | SOCK_CLOEXEC);
	if (client < 0)
		return 0;
	if (getsockopt(client, SOL_SOCKET, SO_PEERCRED, &cred, &cred_len) < 0 ||
	    (cred.uid != 0 && cred.uid != getuid())) {
		close(client);
		return 0;
	}

	close_client(input);
	input->client_fd = client;
	input->client_source = wl_event_loop_add_fd(
		wl_display_get_event_loop(input->compositor->wl_display), client,
		WL_EVENT_READABLE | WL_EVENT_HANGUP | WL_EVENT_ERROR,
		client_ready, input);
	if (!input->client_source)
		close_client(input);
	return 0;
}

static void
destroy_input(struct wl_listener *listener, void *data)
{
	struct splitdesk_input *input =
		wl_container_of(listener, input, destroy_listener);
	(void)data;

	close_client(input);
	if (input->listen_source)
		wl_event_source_remove(input->listen_source);
	if (input->listen_fd >= 0)
		close(input->listen_fd);
	unlink(input->socket_path);
	weston_seat_release(&input->seat);
	free(input);
}

WL_EXPORT int
wet_module_init(struct weston_compositor *compositor, int *argc, char *argv[])
{
	struct splitdesk_input *input;
	struct sockaddr_un address = { .sun_family = AF_UNIX };
	struct wl_event_loop *loop;
	const char *path = getenv("SPLITDESK_INPUT_SOCKET");
	const char *seat_name = getenv("SPLITDESK_SEAT_NAME");
	(void)argc;
	(void)argv;

	if (!path || path[0] != '/' || strlen(path) >= sizeof(address.sun_path)) {
		weston_log("SplitDesk input: invalid SPLITDESK_INPUT_SOCKET\n");
		return -1;
	}
	input = calloc(1, sizeof(*input));
	if (!input)
		return -1;
	input->compositor = compositor;
	input->listen_fd = -1;
	input->client_fd = -1;
	strncpy(input->socket_path, path, sizeof(input->socket_path) - 1);
	strncpy(address.sun_path, path, sizeof(address.sun_path) - 1);

	weston_seat_init(&input->seat, compositor,
			 seat_name && *seat_name ? seat_name : "splitdesk");
	weston_seat_init_pointer(&input->seat);
	if (weston_seat_init_keyboard(&input->seat, NULL) < 0)
		goto fail_seat;

	input->listen_fd = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
	if (input->listen_fd < 0)
		goto fail_seat;
	unlink(path);
	if (bind(input->listen_fd, (struct sockaddr *)&address, sizeof(address)) < 0 ||
	    chmod(path, 0600) < 0 || listen(input->listen_fd, 1) < 0)
		goto fail_socket;

	loop = wl_display_get_event_loop(compositor->wl_display);
	input->listen_source = wl_event_loop_add_fd(
		loop, input->listen_fd, WL_EVENT_READABLE | WL_EVENT_ERROR,
		listen_ready, input);
	if (!input->listen_source)
		goto fail_socket;
	input->destroy_listener.notify = destroy_input;
	wl_signal_add(&compositor->destroy_signal, &input->destroy_listener);
	weston_log("SplitDesk input: compositor-local seat ready at %s\n", path);
	return 0;

fail_socket:
	if (input->listen_fd >= 0)
		close(input->listen_fd);
	unlink(path);
fail_seat:
	weston_seat_release(&input->seat);
	free(input);
	return -1;
}
