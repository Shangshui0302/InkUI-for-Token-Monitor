#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createServer } from "node:http";

const port = Number(process.argv[2] || 18789);
const server = createServer((_, response) => {
  response.writeHead(426);
  response.end("WebSocket upgrade required");
});

server.on("upgrade", (request, socket) => {
  const key = request.headers["sec-websocket-key"];
  if (typeof key !== "string") {
    socket.destroy();
    return;
  }
  const accept = createHash("sha1")
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  socket.write([
    "HTTP/1.1 101 Switching Protocols",
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Accept: ${accept}`,
    "",
    "",
  ].join("\r\n"));

  socket.once("data", (frame) => {
    const masked = (frame[1] & 0x80) !== 0;
    let length = frame[1] & 0x7f;
    let offset = 2;
    if (length === 126) {
      length = frame.readUInt16BE(offset);
      offset += 2;
    }
    if (!masked || length > 65535 || frame.length < offset + 4 + length) {
      socket.destroy();
      return;
    }
    const mask = frame.subarray(offset, offset + 4);
    offset += 4;
    const payload = Buffer.alloc(length);
    for (let index = 0; index < length; index += 1) {
      payload[index] = frame[offset + index] ^ mask[index % 4];
    }
    const response = Buffer.concat([Buffer.from([0x81, payload.length]), payload]);
    socket.end(response);
  });
});

server.listen(port, "127.0.0.1", () => {
  console.log(`P0 WebSocket echo listening on 127.0.0.1:${port}`);
});
