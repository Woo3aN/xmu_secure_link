/* C ABI over the OpenVPN3 core ClientAPI, consumed by the Rust FFI layer.
 * Implemented in ovpn_shim.cpp. All strings are UTF-8, NUL-terminated.
 * Strings returned via char** out-params are heap-allocated and must be
 * released with ovpn_string_free(). */
#ifndef XMU_OVPN_FFI_H
#define XMU_OVPN_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ovpn_client ovpn_client;

/* Delivered for every core log line. */
typedef void (*ovpn_log_cb)(void *user, const char *text);

/* Delivered for every connection event (CONNECTED, DISCONNECTED, AUTH_FAILED, ...). */
typedef void (*ovpn_event_cb)(void *user,
                              int error,
                              int fatal,
                              const char *name,
                              const char *info);

/* Create a client. user is passed back to the callbacks. Returns NULL on OOM. */
ovpn_client *ovpn_client_new(void *user, ovpn_log_cb log_cb, ovpn_event_cb event_cb);

void ovpn_client_free(ovpn_client *client);

/* Parse the profile and (unless autologin) install username/password.
 * peer_keys/peer_vals are peer_count parallel arrays of key/value pairs sent
 * to the server as peer-info (the OpenVPN3 equivalent of push-peer-info +
 * `setenv UV_*`); pass 0/NULL for none.
 * Returns 0 on success, 1 on error with *msg_out set to a message. */
int ovpn_client_prepare(ovpn_client *client,
                        const char *profile,
                        const char *username,
                        const char *password,
                        const char *const *peer_keys,
                        const char *const *peer_vals,
                        int peer_count,
                        char **msg_out);

/* Blocks until the session disconnects. Returns 0 on clean exit, 1 on error
 * with *msg_out set. Safe to call ovpn_client_stop() from another thread. */
int ovpn_client_connect(ovpn_client *client, char **msg_out);

/* Request an orderly disconnect of a running ovpn_client_connect(). */
void ovpn_client_stop(ovpn_client *client);

/* Platform description string from the core. Caller frees with ovpn_string_free. */
char *ovpn_platform(void);

void ovpn_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* XMU_OVPN_FFI_H */
