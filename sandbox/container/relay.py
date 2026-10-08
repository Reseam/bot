import base64
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading

OUTPUT_LIMIT = 1024 * 1024
READ_CHUNK = 64 * 1024


class Relay:
    def __init__(self):
        self.output = threading.Lock()
        self.lock = threading.Lock()
        self.next_id = 1
        self.waiting = {}
        self.call_cwds = {}
        self.images = []
        self.cancelled = threading.Event()

    def send(self, message):
        with self.output:
            sys.stdout.write(json.dumps(message) + "\n")
            sys.stdout.flush()

    def request(self, message, cwd):
        event = threading.Event()
        slot = {}
        with self.lock:
            if self.cancelled.is_set():
                return {"error": "cancelled"}
            request_id = self.next_id
            self.next_id += 1
            self.waiting[request_id] = (event, slot)
            self.call_cwds[request_id] = cwd
        self.send({**message, "id": request_id})
        event.wait()
        with self.lock:
            self.call_cwds.pop(request_id, None)
        return slot["result"]

    def resolve(self, request_id, result):
        with self.lock:
            waiter = self.waiting.pop(request_id, None)
        if waiter:
            event, slot = waiter
            slot["result"] = result
            event.set()

    def cancel(self):
        with self.lock:
            self.cancelled.set()
            waiters = list(self.waiting.values())
            self.waiting.clear()
        for event, slot in waiters:
            slot["result"] = {"error": "cancelled"}
            event.set()

    def read_file(self, message):
        with self.lock:
            cwd = self.call_cwds.get(message["call"])
        try:
            if cwd is None:
                raise OSError("command is no longer active")
            path = os.path.join(cwd, message["path"])
            if not os.path.isfile(path):
                raise OSError("not a regular file")
            if os.path.getsize(path) > message["max_bytes"]:
                raise OSError(f"file exceeds the {message['max_bytes']} byte limit")
            with open(path, "rb") as file:
                data = file.read(message["max_bytes"] + 1)
            if len(data) > message["max_bytes"]:
                raise OSError(f"file exceeds the {message['max_bytes']} byte limit")
            result = {"base64": base64.b64encode(data).decode()}
        except OSError as error:
            result = {"error": error.strerror or str(error)}
        self.send({"type": "file_result", "id": message["id"], "result": result})

    def listen(self, on_cancel):
        for line in sys.stdin:
            message = json.loads(line)
            if message["type"] == "reply":
                self.resolve(message["id"], message["result"])
            elif message["type"] == "read_file":
                self.read_file(message)
            else:
                self.send({
                    "type": "file_result",
                    "id": message["id"],
                    "result": {"error": "not available in this sandbox"},
                })
        self.cancel()
        on_cancel()

    def serve(self, connection):
        with connection, connection.makefile("rwb") as stream:
            request = json.loads(stream.readline())
            cwd = request.pop("cwd")
            if request["type"] == "view":
                with self.lock:
                    self.images.extend(request["images"])
                reply = {"ok": True}
            else:
                reply = self.request(request, cwd)
            stream.write(json.dumps(reply).encode() + b"\n")


class Capture:
    def __init__(self, pipe):
        self.data = bytearray()
        self.size = 0
        self.thread = threading.Thread(target=self.read, args=(pipe,), daemon=True)
        self.thread.start()

    def read(self, pipe):
        while chunk := pipe.read(READ_CHUNK):
            self.size += len(chunk)
            room = OUTPUT_LIMIT - len(self.data)
            if room > 0:
                self.data += chunk[:room]

    def result(self):
        self.thread.join(timeout=2)
        return base64.b64encode(bytes(self.data)).decode(), self.size


def accept(server, relay):
    while True:
        connection, _ = server.accept()
        threading.Thread(target=relay.serve, args=(connection,), daemon=True).start()


def kill(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def main():
    start = json.loads(sys.stdin.readline())
    relay = Relay()
    directory = tempfile.mkdtemp(prefix="reseam-bridge-")
    address = os.path.join(directory, "socket")
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(address)
    server.listen()
    threading.Thread(target=accept, args=(server, relay), daemon=True).start()

    environment = dict(
        os.environ,
        RESEAM_BRIDGE=address,
        PATH=f"{start['path']}:{os.environ['PATH']}",
    )
    process = subprocess.Popen(
        ["bash", "-c", start["command"]],
        cwd=start["cwd"],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    stdout = Capture(process.stdout)
    stderr = Capture(process.stderr)
    threading.Thread(target=relay.listen, args=(lambda: kill(process),), daemon=True).start()

    timed_out = False
    try:
        process.wait(timeout=start["timeout_ms"] / 1000)
    except subprocess.TimeoutExpired:
        timed_out = True
    kill(process)
    exit_code = process.wait()
    stdout_base64, stdout_size = stdout.result()
    stderr_base64, stderr_size = stderr.result()
    shutil.rmtree(directory, ignore_errors=True)
    with relay.lock:
        images = list(relay.images)
    relay.send({
        "type": "exec_result",
        "stdout": stdout_base64,
        "stdout_size": stdout_size,
        "stderr": stderr_base64,
        "stderr_size": stderr_size,
        "exit_code": 128 - exit_code if exit_code < 0 else exit_code,
        "timed_out": timed_out,
        "cancelled": relay.cancelled.is_set(),
        "images": images,
    })


main()
