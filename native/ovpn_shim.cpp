// C++ shim that subclasses the OpenVPN3 core ClientAPI and exposes the C ABI
// declared in ovpn_ffi.h. Mirrors the structure of test/ovpncli/cli.cpp but
// routes events/logs to C callbacks instead of stdout.

#include "ovpncli.hpp"

#include <cstdlib>
#include <cstring>
#include <exception>
#include <string>

#include "ovpn_ffi.h"

using namespace openvpn::ClientAPI;

namespace {

char *dup_cstr(const std::string &s)
{
    char *p = static_cast<char *>(std::malloc(s.size() + 1));
    if (p)
        std::memcpy(p, s.c_str(), s.size() + 1);
    return p;
}

// Subclass of the abstract OpenVPN3 client. Only the pure-virtual hooks are
// overridden; tun/transport setup falls back to the platform default (native
// tun on desktop, which requires administrator privileges).
class FfiClient : public OpenVPNClient
{
  public:
    FfiClient(void *user, ovpn_log_cb log_cb, ovpn_event_cb event_cb)
        : user_(user), log_cb_(log_cb), event_cb_(event_cb)
    {
    }

    void event(const Event &ev) override
    {
        if (event_cb_)
            event_cb_(user_, ev.error ? 1 : 0, ev.fatal ? 1 : 0, ev.name.c_str(), ev.info.c_str());
    }

    void acc_event(const AppCustomControlMessageEvent &) override
    {
    }

    void log(const LogInfo &li) override
    {
        if (log_cb_)
            log_cb_(user_, li.text.c_str());
    }

    void external_pki_cert_request(ExternalPKICertRequest &req) override
    {
        req.error = true;
        req.errorText = "external PKI is not supported by this client";
    }

    void external_pki_sign_request(ExternalPKISignRequest &req) override
    {
        req.error = true;
        req.errorText = "external PKI is not supported by this client";
    }

    bool pause_on_connection_timeout() override
    {
        return false;
    }

  private:
    void *user_;
    ovpn_log_cb log_cb_;
    ovpn_event_cb event_cb_;
};

} // namespace

// Holds the client plus an OpenVPNClientHelper, whose lifetime keeps the core
// InitProcess (crypto/process init) alive for as long as the client exists.
struct ovpn_client
{
    OpenVPNClientHelper helper;
    FfiClient client;

    ovpn_client(void *user, ovpn_log_cb log_cb, ovpn_event_cb event_cb)
        : client(user, log_cb, event_cb)
    {
    }
};

extern "C" {

ovpn_client *ovpn_client_new(void *user, ovpn_log_cb log_cb, ovpn_event_cb event_cb)
{
    try
    {
        return new ovpn_client(user, log_cb, event_cb);
    }
    catch (...)
    {
        return nullptr;
    }
}

void ovpn_client_free(ovpn_client *client)
{
    delete client;
}

int ovpn_client_prepare(ovpn_client *client,
                        const char *profile,
                        const char *username,
                        const char *password,
                        const char *const *peer_keys,
                        const char *const *peer_vals,
                        int peer_count,
                        char **msg_out)
{
    if (msg_out)
        *msg_out = nullptr;
    if (!client)
        return 1;
    try
    {
        Config config;
        config.content = profile ? profile : "";
        config.guiVersion = "xmu_secure_link 0.1";
        config.info = true;
        config.echo = true;
        config.tunPersist = false;
        // SecureLink authenticates with username/password only (server CA, no
        // client cert/key), so suppress the client cert to avoid the core
        // treating the profile as an external-PKI profile.
        config.disableClientCert = true;
        for (int i = 0; i < peer_count; ++i)
        {
            const char *k = peer_keys ? peer_keys[i] : nullptr;
            const char *v = peer_vals ? peer_vals[i] : nullptr;
            if (k)
                config.peerInfo.emplace_back(k, v ? v : "");
        }

        EvalConfig eval = client->client.eval_config(config);
        if (eval.error)
        {
            if (msg_out)
                *msg_out = dup_cstr(eval.message);
            return 1;
        }

        if (!eval.autologin)
        {
            ProvideCreds creds;
            creds.username = username ? username : "";
            creds.password = password ? password : "";
            Status st = client->client.provide_creds(creds);
            if (st.error)
            {
                if (msg_out)
                    *msg_out = dup_cstr(st.message.empty() ? st.status : st.message);
                return 1;
            }
        }
        return 0;
    }
    catch (const std::exception &e)
    {
        if (msg_out)
            *msg_out = dup_cstr(e.what());
        return 1;
    }
}

int ovpn_client_connect(ovpn_client *client, char **msg_out)
{
    if (msg_out)
        *msg_out = nullptr;
    if (!client)
        return 1;
    try
    {
        Status st = client->client.connect();
        if (st.error)
        {
            if (msg_out)
                *msg_out = dup_cstr(st.message.empty() ? st.status : st.message);
            return 1;
        }
        return 0;
    }
    catch (const std::exception &e)
    {
        if (msg_out)
            *msg_out = dup_cstr(e.what());
        return 1;
    }
}

void ovpn_client_stop(ovpn_client *client)
{
    if (!client)
        return;
    try
    {
        client->client.stop();
    }
    catch (...)
    {
    }
}

char *ovpn_platform(void)
{
    try
    {
        return dup_cstr(OpenVPNClientHelper::platform());
    }
    catch (...)
    {
        return nullptr;
    }
}

void ovpn_string_free(char *s)
{
    std::free(s);
}

} // extern "C"
