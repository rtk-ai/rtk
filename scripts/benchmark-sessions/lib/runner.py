from __future__ import annotations

import asyncio
import subprocess
import tempfile
from pathlib import Path

from .config import TaskConfig
from .manifest import (
    RunManifest,
    SessionEntry,
    TbEntry,
    TbTaskEntry,
    write_manifest,
)
from .session import run_all_sessions, setup_codebase, setup_rtk
from .terminal_bench import run_terminal_bench
from .vm import create_vm_pool, destroy_vm_pool

ROOT_DIR = Path(__file__).resolve().parent.parent


def _create_tarball(source_dir: Path) -> str:
    tarball = tempfile.mktemp(suffix=".tar.gz")
    subprocess.run(
        ["tar", "czf", tarball, "-C", str(source_dir), "."],
        check=True,
    )
    return tarball


def _print_step(step: int, total: int, msg: str):
    print(f"\n[{step}/{total}] {msg}")


def _session_to_entry(r) -> SessionEntry:
    return SessionEntry(
        vm_name=r.vm_name,
        group=r.group,
        stdout_json=f"{r.vm_name}-stdout.json",
        otel_log=f"{r.vm_name}-otel.log",
        rtk_db=f"{r.vm_name}-tracking.db" if r.rtk_db_path else None,
        exit_code=r.exit_code,
        error=r.error or None,
    )


def _tb_to_entry(r) -> TbEntry:
    return TbEntry(
        vm_name=r.vm_name,
        group=r.group,
        total=r.total,
        passed=r.passed,
        failed=r.failed,
        tasks=[TbTaskEntry(name=t.name, passed=t.passed, duration_s=t.duration_s) for t in r.tasks],
        error=r.error,
    )


async def run_benchmark(
    task: TaskConfig,
    vms: int,
    api_key: str,
    output_dir: Path,
    cloud_init: Path | None = None,
    terminal_bench: bool = False,
    keep_vms: bool = False,
) -> RunManifest:
    if cloud_init is None:
        cloud_init = ROOT_DIR / "cloud-init-base.yaml"

    output_dir.mkdir(parents=True, exist_ok=True)

    total_steps = 5 if terminal_bench else 4
    vm_names: list[str] = []

    manifest = RunManifest(
        task_name=task.name,
        model=task.model,
        vm_count=vms,
    )

    try:
        _print_step(1, total_steps, f"Creating {vms * 2} VMs ({vms} RTK ON + {vms} RTK OFF)")
        vm_names = await create_vm_pool(vms, cloud_init)
        print(f"  VMs ready: {', '.join(vm_names)}")

        _print_step(2, total_steps, "Setting up codebases")
        local_tarball = None
        if not task.codebase.is_github:
            local_tarball = _create_tarball(task.codebase.local_path())

        await asyncio.gather(*(
            setup_codebase(name, task.codebase, local_tarball)
            for name in vm_names
        ))
        print("  Codebases deployed")

        _print_step(3, total_steps, "Configuring RTK on ON VMs")
        setup_script = ROOT_DIR / "setup-rtk.sh"
        on_vms = [n for n in vm_names if "-on-" in n]
        off_vms = [n for n in vm_names if "-off-" in n]
        await asyncio.gather(*(setup_rtk(vm, setup_script) for vm in on_vms))
        print(f"  RTK configured on {len(on_vms)} VMs")

        _print_step(4, total_steps, f"Running Claude sessions (timeout: {task.timeout_minutes}min)")
        results = await run_all_sessions(vm_names, task, api_key, output_dir)

        on_ok = [r for r in results if r.group == "on" and not r.error]
        off_ok = [r for r in results if r.group == "off" and not r.error]
        errors = [r for r in results if r.error]
        print(f"  Completed: {len(on_ok)} ON, {len(off_ok)} OFF, {len(errors)} errors")
        for r in errors:
            print(f"    {r.vm_name}: {r.error}")

        manifest.sessions = [_session_to_entry(r) for r in results]

        if terminal_bench:
            _print_step(5, total_steps, "Running terminal-bench precision tests")
            tb_on = await asyncio.gather(*(
                run_terminal_bench(vm, "on", task.model, api_key)
                for vm in on_vms
            ))
            tb_off = await asyncio.gather(*(
                run_terminal_bench(vm, "off", task.model, api_key)
                for vm in off_vms
            ))

            manifest.terminal_bench = [_tb_to_entry(r) for r in list(tb_on) + list(tb_off)]

            ok_on = [r for r in tb_on if not r.error]
            ok_off = [r for r in tb_off if not r.error]
            if ok_on and ok_off:
                on_total = sum(r.total for r in ok_on)
                on_passed = sum(r.passed for r in ok_on)
                off_total = sum(r.total for r in ok_off)
                off_passed = sum(r.passed for r in ok_off)
                on_rate = on_passed / on_total if on_total else 0
                off_rate = off_passed / off_total if off_total else 0
                print(f"  terminal-bench: ON pass rate={on_rate:.0%}, OFF pass rate={off_rate:.0%}, delta={on_rate - off_rate:+.0%}")

            tb_errors = [r for r in list(tb_on) + list(tb_off) if r.error]
            for r in tb_errors:
                print(f"    {r.vm_name}: {r.error}")

        write_manifest(manifest, output_dir)
        print(f"\n  Manifest written to {output_dir / 'manifest.json'}")

    finally:
        if not keep_vms and vm_names:
            print("\nCleaning up VMs...")
            await destroy_vm_pool(vm_names)
            print("  VMs destroyed")

    return manifest
