import base64
import json
import os
import socket
import sys

FETCH_USAGE = """usage: fetch [--method METHOD] [--header 'Name: value']... [--body FILE] URL
Make an HTTP request through the bot. Requests to configured forge APIs are authenticated.
--body - reads the body from stdin. Prints the response body and exits 22 on an HTTP error status."""
VIEW_USAGE = "usage: view FILE..."


def exchange(request):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.connect(os.environ["RESEAM_BRIDGE"])
        with connection.makefile("rwb") as stream:
            stream.write(json.dumps({**request, "cwd": os.getcwd()}).encode() + b"\n")
            stream.flush()
            return json.loads(stream.readline())


def fail(name, message, code=1):
    sys.stderr.write(f"{name}: {message}\n")
    return code


def view(args):
    if not args or "--help" in args or "-h" in args:
        sys.stdout.write(VIEW_USAGE + "\n")
        return 0 if args else 2
    images = []
    for path in args:
        try:
            with open(path, "rb") as file:
                images.append({"name": path, "base64": base64.b64encode(file.read()).decode()})
        except OSError as error:
            return fail("view", f"{path}: {error.strerror}")
    exchange({"type": "view", "images": images})
    sys.stdout.write(f"Attached {', '.join(args)} to this result.\n")
    return 0


def fetch_request(args):
    method, headers, body, positional = "GET", {}, None, []
    rest = list(args)
    while rest:
        arg = rest.pop(0)
        if arg in ("--method", "-X") and rest:
            method = rest.pop(0).upper()
        elif arg in ("--header", "-H") and rest:
            header = rest.pop(0)
            name, separator, value = header.partition(":")
            if not separator or not name.strip():
                raise ValueError(f"invalid header: {header}")
            headers[name.strip().lower()] = value.strip()
        elif arg == "--body" and rest:
            path = rest.pop(0)
            if path == "-":
                body = sys.stdin.buffer.read()
            else:
                with open(path, "rb") as file:
                    body = file.read()
        elif arg.startswith("-"):
            raise ValueError(f"unknown option: {arg}")
        else:
            positional.append(arg)
    if len(positional) != 1:
        raise ValueError("expected one URL")
    return {
        "type": "fetch",
        "url": positional[0],
        "method": method,
        "headers": headers,
        "body_base64": None if body is None else base64.b64encode(body).decode(),
    }


def fetch(args):
    if "--help" in args or "-h" in args:
        sys.stdout.write(FETCH_USAGE + "\n")
        return 0
    try:
        request = fetch_request(args)
    except (ValueError, OSError) as error:
        return fail("fetch", f"{error}\n{FETCH_USAGE}", 2)
    reply = exchange(request)
    if "error" in reply:
        return fail("fetch", reply["error"])
    response = reply["fetch"]
    sys.stdout.buffer.write(base64.b64decode(response["body_base64"]))
    if response["status"] >= 400:
        return fail("fetch", f"HTTP {response['status']} {response['status_text']}", 22)
    return 0


def call(name, args):
    stdin = "" if sys.stdin.isatty() else sys.stdin.read()
    reply = exchange({"type": "call", "command": name, "args": args, "stdin": stdin})
    if "error" in reply:
        return fail(name, reply["error"])
    output = reply["command"]
    if output["file"]:
        try:
            with open(output["file"]["path"], "wb") as file:
                file.write(base64.b64decode(output["file"]["base64"]))
        except OSError as error:
            return fail(name, f"{output['file']['path']}: {error.strerror}")
    sys.stdout.write(output["stdout"])
    sys.stderr.write(output["stderr"])
    return output["exit_code"]


def main():
    name, args = sys.argv[1], sys.argv[2:]
    if name == "view":
        return view(args)
    if name == "fetch":
        return fetch(args)
    return call(name, args)


sys.exit(main())
