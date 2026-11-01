#!/usr/bin/env python3
"""Loopback OpenAI-compatible proxy that adds the provider session header.

CodeBoarding accepts any OpenAI-compatible `OPENAI_BASE_URL` but has no setting
for extra headers. The authorized provider (`opencode go orange`) requires
`x-opencode-session`. This proxy forwards `/v1/*` to the provider, injects the
configured headers and the real bearer key, and stays on 127.0.0.1.

The key is read from the ZCode provider config and never printed, logged or
placed in argv. Only method, path, status and duration are logged.
"""

from __future__ import annotations

import argparse
import http.client
import http.server
import json
import os
import ssl
import sys
import threading
import time
import urllib.parse


def load_provider(config_path: str, provider_name: str) -> tuple[str, str, dict[str, str]]:
    with open(config_path, encoding="utf-8") as handle:
        config = json.load(handle)
    rules = config["config"]["providerConfigRules"]["providerRules"]
    for rule in rules:
        if rule.get("providerName") == provider_name:
            provider = rule["config"]
            access = provider.get("access", {})
            key = access.get("apiKey", "")
            api = provider.get("api", {})
            base = (api.get("baseUrl") or "").rstrip("/")
            headers = dict(api.get("headers") or {})
            if access.get("type") != "api-key" or not key:
                raise SystemExit("provider does not use an API key")
            if not base.startswith(("https://", "http://")):
                raise SystemExit("provider base URL is not http(s)")
            return key, base, headers
    raise SystemExit(f"provider {provider_name!r} not found in {config_path}")


class Proxy:
    def __init__(self, key: str, base: str, extra_headers: dict[str, str]) -> None:
        self.key = key
        self.base = urllib.parse.urlsplit(base)
        self.extra_headers = extra_headers
        self.calls = 0
        self.lock = threading.Lock()

    def forward(self, method: str, path: str, body: bytes | None, content_type: str):
        suffix = path[len("/v1") :]
        target_path = self.base.path.rstrip("/") + suffix
        connection_class = (
            http.client.HTTPSConnection
            if self.base.scheme == "https"
            else http.client.HTTPConnection
        )
        kwargs = {"timeout": 300}
        if self.base.scheme == "https":
            kwargs["context"] = ssl.create_default_context()
        connection = connection_class(self.base.hostname, self.base.port, **kwargs)
        headers = {
            "content-type": content_type or "application/json",
            "authorization": f"Bearer {self.key}",
            "accept": "application/json",
        }
        headers.update(self.extra_headers)
        started = time.time()
        try:
            connection.request(method, target_path, body=body, headers=headers)
            response = connection.getresponse()
            data = response.read()
        finally:
            connection.close()
        with self.lock:
            self.calls += 1
            call_number = self.calls
        print(
            json.dumps(
                {
                    "call": call_number,
                    "method": method,
                    "path": suffix,
                    "status": response.status,
                    "duration_ms": round((time.time() - started) * 1000, 1),
                    "bytes": len(data),
                }
            ),
            file=sys.stderr,
            flush=True,
        )
        return response.status, response.getheaders(), data


def handler_for(proxy: Proxy):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format, *args):  # noqa: A002 - silence default logging
            return

        def do_GET(self):
            self._forward("GET")

        def do_POST(self):
            self._forward("POST")

        def _forward(self, method: str):
            parsed = urllib.parse.urlsplit(self.path)
            if not parsed.path.startswith("/v1/"):
                self.send_response(404)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            length = int(self.headers.get("content-length") or 0)
            body = self.rfile.read(length) if length else None
            try:
                status, headers, data = proxy.forward(
                    method, parsed.path, body, self.headers.get("content-type", "")
                )
            except Exception as error:  # noqa: BLE001 - report upstream failure
                message = json.dumps({"error": f"proxy upstream failure: {type(error).__name__}"}).encode()
                self.send_response(502)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(message)))
                self.end_headers()
                self.wfile.write(message)
                return
            self.send_response(status)
            for name, value in headers:
                if name.lower() in {
                    "connection",
                    "transfer-encoding",
                    "content-length",
                    "keep-alive",
                    "content-encoding",
                }:
                    # http.client already decodes content-encoding when reading.
                    continue
                self.send_header(name, value)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    return Handler


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--provider-config",
        default=os.path.expandvars(r"%USERPROFILE%\.zcode\v2\provider_config.json"),
    )
    parser.add_argument("--provider-name", default="opencode go orange")
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    key, base, headers = load_provider(args.provider_config, args.provider_name)
    proxy = Proxy(key, base, headers)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(proxy))
    server.daemon_threads = True
    port = server.server_address[1]
    print(
        json.dumps(
            {
                "endpoint": f"http://127.0.0.1:{port}/v1",
                "provider": args.provider_name,
                "model": "deepseek-v4.1-flash",
                "base_url_host": urllib.parse.urlsplit(base).hostname,
                "key_source": "zcode provider config",
            }
        ),
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
