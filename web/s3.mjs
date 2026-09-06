// SigV4 range GETs against S3, as a fetcher for `CachedBytes`.
//
// The generic half of the cached byte source -- ranges, caching, eviction,
// the budget -- lives in `cache.mjs` and takes any
// `(offset, length) -> {bytes, total}` function.
// The only AWS-specific part is signing, so this is that function for S3: a
// deliberately minimal SigV4 signer for exactly one request shape, GET with a
// Range header. No dependencies; WebCrypto does the HMACs. It ships with the
// engine anyway because it is the obvious first user of the seam, and there
// is no good reason to make every S3 caller re-derive canonical-request
// ordering from the AWS docs.
//
// What stays with the caller: credentials, the bucket, and which
// object to open. Credentials live in this closure for the life of the
// fetcher and are never persisted -- the cache stores bytes, not requests,
// and nothing here writes a credential anywhere.

const enc = new TextEncoder();

function hex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function hmac(key, msg) {
  const k = await crypto.subtle.importKey("raw", key, { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", k, enc.encode(msg)));
}

async function sha256hex(msg) {
  return hex(await crypto.subtle.digest("SHA-256", enc.encode(msg)));
}

/// A `CachedBytes` fetcher for one S3 object.
///
///   s3RangeFetcher({
///     region, bucket, key,
///     accessKeyId, secretAccessKey,
///     sessionToken,          // optional, for STS credentials
///     endpoint,              // optional, e.g. a LocalStack or R2 URL;
///                            // defaults to virtual-hosted-style AWS
///   })
///
/// The object must be publicly consistent for the life of the cache -- which
/// an immutable day index is by construction. Name the cache after the
/// object version so a re-rolled object never resumes over stale pages.
export function s3RangeFetcher({
  region,
  bucket,
  key,
  accessKeyId,
  secretAccessKey,
  sessionToken,
  endpoint,
}) {
  const base = endpoint
    ? `${endpoint.replace(/\/$/, "")}/${bucket}`
    : `https://${bucket}.s3.${region}.amazonaws.com`;
  const host = new URL(base).host;
  const path = `${new URL(base).pathname.replace(/\/$/, "")}/${key}`
    // S3 signs the URI-encoded path, segment by segment, slashes preserved.
    .split("/")
    .map((s) => encodeURIComponent(s))
    .join("/");

  return async (off, len) => {
    const now = new Date();
    const amzDate = now.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}/, "");
    const day = amzDate.slice(0, 8);
    const scope = `${day}/${region}/s3/aws4_request`;
    const range = `bytes=${off}-${off + len - 1}`;

    // Canonical request: sorted, lowercase headers; UNSIGNED-PAYLOAD because
    // a GET sends none and hashing nothing is what that token is for.
    const headers = [
      ["host", host],
      ["range", range],
      ["x-amz-content-sha256", "UNSIGNED-PAYLOAD"],
      ["x-amz-date", amzDate],
    ];
    if (sessionToken) headers.push(["x-amz-security-token", sessionToken]);
    headers.sort((a, b) => (a[0] < b[0] ? -1 : 1));
    const signedHeaders = headers.map(([k]) => k).join(";");
    const canonical = [
      "GET",
      path,
      "",
      ...headers.map(([k, v]) => `${k}:${v}`),
      "",
      signedHeaders,
      "UNSIGNED-PAYLOAD",
    ].join("\n");
    const toSign = ["AWS4-HMAC-SHA256", amzDate, scope, await sha256hex(canonical)].join("\n");

    let k = await hmac(enc.encode(`AWS4${secretAccessKey}`), day);
    k = await hmac(k, region);
    k = await hmac(k, "s3");
    k = await hmac(k, "aws4_request");
    const signature = hex(await hmac(k, toSign));

    const req = {
      headers: {
        range,
        "x-amz-content-sha256": "UNSIGNED-PAYLOAD",
        "x-amz-date": amzDate,
        authorization:
          `AWS4-HMAC-SHA256 Credential=${accessKeyId}/${scope}, ` +
          `SignedHeaders=${signedHeaders}, Signature=${signature}`,
      },
    };
    if (sessionToken) req.headers["x-amz-security-token"] = sessionToken;

    const res = await fetch(`${base}/${key}`, req);
    if (res.status !== 206) {
      throw new Error(`supdb s3: ${bucket}/${key} answered ${res.status} to a range request`);
    }
    const m = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(res.headers.get("content-range") ?? "");
    if (!m || Number(m[1]) !== off) {
      throw new Error(`supdb s3: unusable content-range for ${off}+${len}`);
    }
    return { bytes: new Uint8Array(await res.arrayBuffer()), total: Number(m[3]) };
  };
}
