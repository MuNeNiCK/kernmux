import { afterEach, describe, expect, it, vi } from "vitest"
import { ApiError, KernmuxApi } from "./api"

afterEach(() => vi.unstubAllGlobals())

describe("KernmuxApi", () => {
  it("surfaces structured API errors", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: { code: "stale_generation", message: "Refresh required" } }), { status: 409 })))
    await expect(new KernmuxApi("token").host()).rejects.toEqual(new ApiError(409, "stale_generation", "Refresh required"))
  })
  it("parses gateway string errors", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 })))
    await expect(new KernmuxApi("token").host()).rejects.toEqual(new ApiError(401, "unauthorized", "unauthorized"))
  })
  it("sends lifecycle preconditions with bearer authorization", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ kind: "accepted", operation: { id: "op-1", state: "queued" } }), { status: 202 }))
    vi.stubGlobal("fetch", fetchMock)
    await new KernmuxApi("secret").lifecycle(2, "start", 7)
    expect(fetchMock).toHaveBeenCalledWith("/api/1.0/instances/2/start", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ expected_generation: 7 }),
      headers: expect.objectContaining({ Authorization: "Bearer secret", "Content-Type": "application/json" }),
    }))
  })

  it("sends instance and image workflow payloads to the versioned API", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ kind: "accepted", operation: { id: "op-1", state: "queued" } }), { status: 202 }))
    vi.stubGlobal("fetch", fetchMock)
    const api = new KernmuxApi("secret")

    await api.createInstance({ expected_generation: 4, id: 3, name: "build", cpu_hardware_ids: [8, 9], memory_bytes: 536870912 })
    await api.updateInstance(3, { expected_generation: 5, cpu_hardware_ids: [10, 11], memory_bytes: 1073741824, dry_run: true })
    await api.loadManagedImage(3, { expected_generation: 5, kernel_id: "sha256:kernel", initrd_id: "sha256:initrd", command_line: "console=mktty0" })
    await api.importImage({ expected_generation: 5, kind: "kernel", source_path: "/var/lib/kernmux/import/vmlinuz", expected_id: "sha256:image" })
    await api.deleteInstance(3, 6)

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/1.0/instances", expect.objectContaining({ method: "POST", body: JSON.stringify({ expected_generation: 4, id: 3, name: "build", cpu_hardware_ids: [8, 9], memory_bytes: 536870912 }) }))
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/1.0/instances/3", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ expected_generation: 5, cpu_hardware_ids: [10, 11], memory_bytes: 1073741824, dry_run: true }) }))
    expect(fetchMock).toHaveBeenNthCalledWith(3, "/api/1.0/instances/3/load-image", expect.objectContaining({ method: "POST", body: JSON.stringify({ expected_generation: 5, kernel_id: "sha256:kernel", initrd_id: "sha256:initrd", command_line: "console=mktty0" }) }))
    expect(fetchMock).toHaveBeenNthCalledWith(4, "/api/1.0/images", expect.objectContaining({ method: "POST", body: JSON.stringify({ expected_generation: 5, kind: "kernel", source_path: "/var/lib/kernmux/import/vmlinuz", expected_id: "sha256:image" }) }))
    expect(fetchMock).toHaveBeenNthCalledWith(5, "/api/1.0/instances/3", expect.objectContaining({ method: "DELETE", body: JSON.stringify({ expected_generation: 6 }) }))
  })

  it("uploads a local OS image as multipart data without a JSON content type", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ kind: "accepted", operation: { id: "op-upload", state: "queued" } }), { status: 202 }))
    vi.stubGlobal("fetch", fetchMock)
    const file = new File([new Uint8Array([0x51, 0x46, 0x49, 0xfb])], "ubuntu.qcow2")

    await new KernmuxApi("secret").uploadOsImage({ expected_generation: 7, file, label: "Ubuntu 24.04", architecture: "x86_64", expected_sha256: "a".repeat(64) })

    const [path, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(path).toBe("/api/1.0/os-images/upload")
    expect(init.method).toBe("POST")
    expect(init.headers).toEqual({ Authorization: "Bearer secret" })
    expect(init.body).toBeInstanceOf(FormData)
    const body = init.body as FormData
    expect((body.get("file") as File).name).toBe("ubuntu.qcow2")
    expect(body.get("label")).toBe("Ubuntu 24.04")
    expect(body.get("expected_generation")).toBe("7")
    expect(body.get("architecture")).toBe("x86_64")
  })
})
