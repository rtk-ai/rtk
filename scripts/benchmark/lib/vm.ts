/**
 * Multipass VM management for RTK integration testing.
 */

import { $ } from "bun";

const VM_NAME = "rtk-test";
const CLOUD_INIT = "scripts/benchmark/cloud-init.yaml";

export interface VmInfo {
  name: string;
  state: string;
  ipv4: string;
}

/** Check if VM exists and is running */
export async function vmExists(): Promise<boolean> {
  const result = await $`multipass list --format json`.quiet();
  const data = JSON.parse(result.stdout.toString());
  return data.list?.some((vm: VmInfo) => vm.name === VM_NAME) ?? false;
}

/** Check if VM is running */
export async function vmRunning(): Promise<boolean> {
  const result = await $`multipass list --format json`.quiet();
  const data = JSON.parse(result.stdout.toString());
  const vm = data.list?.find((v: VmInfo) => v.name === VM_NAME);
  return vm?.state === "Running";
}

/** Create a new VM with cloud-init (20 min timeout for full provisioning) */
export async function vmCreate(): Promise<void> {
  console.log(`[vm] Creating ${VM_NAME} with cloud-init (this takes ~10-15 min)...`);
  // --timeout 1200 = 20 min for cloud-init to finish installing Rust, Go, Node, .NET, etc.
  await $`multipass launch --name ${VM_NAME} --cpus 2 --memory 4G --disk 20G --timeout 1200 --cloud-init ${CLOUD_INIT} 24.04`;
}

/** Start existing VM */
export async function vmStart(): Promise<void> {
  console.log(`[vm] Starting ${VM_NAME}...`);
  await $`multipass start ${VM_NAME}`;
}

/** Execute a command in the VM, returns stdout (60s timeout per test by default) */
export async function vmExec(
  cmd: string,
  timeoutMs = 60_000
): Promise<{
  stdout: string;
  stderr: string;
  exitCode: number;
}> {
  const exec = $`multipass exec ${VM_NAME} -- bash -c ${cmd}`
    .quiet()
    .nothrow()
    .then((r) => ({
      stdout: r.stdout.toString(),
      stderr: r.stderr.toString(),
      exitCode: r.exitCode,
    }));

  const timeout = new Promise<{ stdout: string; stderr: string; exitCode: number }>((_, reject) =>
    setTimeout(() => reject(new Error(`vmExec timed out after ${timeoutMs}ms: ${cmd}`)), timeoutMs)
  );

  return Promise.race([exec, timeout]);
}

/** Transfer a file to the VM */
export async function vmTransfer(
  localPath: string,
  remotePath: string
): Promise<void> {
  await $`multipass transfer ${localPath} ${VM_NAME}:${remotePath}`;
}

/** Wait for cloud-init to complete (max 40 min — installs Rust, Go, Node, .NET, etc.) */
export async function vmWaitReady(maxWaitSec = 2400): Promise<boolean> {
  console.log("[vm] Waiting for cloud-init...");
  const start = Date.now();
  while ((Date.now() - start) / 1000 < maxWaitSec) {
    const { exitCode } = await vmExec(
      "test -f /home/ubuntu/.cloud-init-complete"
    );
    if (exitCode === 0) {
      const elapsed = Math.round((Date.now() - start) / 1000);
      console.log(`[vm] Cloud-init complete after ${elapsed}s`);
      return true;
    }
    await Bun.sleep(10_000);
  }
  console.error("[vm] Cloud-init timed out!");
  return false;
}

/** Transfer RTK source and build in release mode */
export async function vmBuildRtk(projectRoot: string): Promise<{
  buildTime: number;
  binarySize: number;
  version: string;
}> {
  console.log("[vm] Transferring RTK source...");

  // Create tarball excluding heavy dirs and macOS resource forks (._*)
  await $`COPYFILE_DISABLE=1 tar czf /tmp/rtk-src.tar.gz --exclude target --exclude .git --exclude node_modules --exclude "index.html*" --exclude "._*" -C ${projectRoot} .`;
  await vmTransfer("/tmp/rtk-src.tar.gz", "/tmp/rtk-src.tar.gz");
  await vmExec(
    "mkdir -p /home/ubuntu/rtk && cd /home/ubuntu/rtk && tar xzf /tmp/rtk-src.tar.gz"
  );

  console.log("[vm] Building RTK (release)...");
  const start = Date.now();
  const { stdout, exitCode } = await vmExec(
    "export PATH=$HOME/.cargo/bin:$PATH && cd /home/ubuntu/rtk && cargo build --release 2>&1 | tail -5"
  );
  const buildTime = Math.round((Date.now() - start) / 1000);

  if (exitCode !== 0) {
    throw new Error(`Build failed:\n${stdout}`);
  }

  const { stdout: sizeStr } = await vmExec(
    "stat -c%s /home/ubuntu/rtk/target/release/rtk"
  );
  const binarySize = parseInt(sizeStr.trim(), 10);

  const { stdout: version } = await vmExec(
    "/home/ubuntu/rtk/target/release/rtk --version"
  );

  console.log(
    `[vm] Build OK in ${buildTime}s — ${binarySize} bytes — ${version.trim()}`
  );

  return { buildTime, binarySize, version: version.trim() };
}

/** Delete the VM */
export async function vmDelete(): Promise<void> {
  console.log(`[vm] Deleting ${VM_NAME}...`);
  await $`multipass delete ${VM_NAME} --purge`.nothrow();
}

/** Ensure VM is ready (create or reuse) */
export async function vmEnsureReady(): Promise<void> {
  if (await vmExists()) {
    if (!(await vmRunning())) {
      await vmStart();
    }
    console.log(`[vm] Reusing existing VM ${VM_NAME}`);
    // Check if cloud-init is still running
    const { exitCode } = await vmExec(
      "test -f /home/ubuntu/.cloud-init-complete"
    );
    if (exitCode !== 0) {
      console.log("[vm] Cloud-init still running, waiting...");
      const ready = await vmWaitReady();
      if (!ready) {
        throw new Error(
          "Cloud-init timed out. Check: multipass exec rtk-test -- cat /var/log/cloud-init-output.log"
        );
      }
    }
  } else {
    await vmCreate();
    // multipass launch --timeout should wait, but double-check
    const { exitCode } = await vmExec(
      "test -f /home/ubuntu/.cloud-init-complete"
    );
    if (exitCode !== 0) {
      const ready = await vmWaitReady();
      if (!ready) {
        throw new Error(
          "Cloud-init timed out. Check: multipass exec rtk-test -- cat /var/log/cloud-init-output.log"
        );
      }
    }
  }
}

export const RTK_BIN = "/home/ubuntu/rtk/target/release/rtk";
