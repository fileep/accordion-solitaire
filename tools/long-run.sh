#!/bin/sh
# long-run.sh: sees a long `acc batch` through, as tools/long-run.ps1 does on Windows. NOT TESTED ON LINUX OR MACOS
# YET: it follows the PowerShell script, which has been tested on Windows.
#
# For most people `acc run` does what this script does, and more (it also pauses a laptop's run while the battery is
# low): see docs/LONG_RUNS.md. This script is for seeing a long `acc batch` through with a binary of your choosing.
#
# It starts the run, or carries on with it if its checkpoint is there, and carries on again whenever it stops for a
# reason that a new process may get past. Run it from the repository's folder, as the run's paths are relative to
# where it started. Press Ctrl+C once to pause the run; this script stops with it.
#
#   sh tools/long-run.sh --acc ./target/release/acc --from 0 --count 1000000000
#   sh tools/long-run.sh --acc ./target/release/acc --from 0 --count 1000000000 --stop-after-hours 8
#   sh tools/long-run.sh --acc ./target/release/acc --from 0 --count 1000000000 --gpu on -- --threads 4
#
# --acc FILE, --from N and --count N are needed. --gpu auto|on|off (auto) is `acc batch --gpu` when the run starts:
# auto uses an NVIDIA card if it can and the CPU alone if not, on refuses to start without the card. Each session
# pauses after --stop-after-hours H, or once --until N of the run's deals are done. --max-failures N (6) tries in a
# row without progress, --wait-seconds S (60) apart, before giving up. What follows -- goes to `acc batch` when the
# run starts, not when it carries on: its settings are in the checkpoint.
#
# acc doesn't keep Linux or macOS from going to sleep. To keep the computer awake for the run, start this script
# under systemd-inhibit (Linux) or caffeinate -i (macOS): systemd-inhibit sh tools/long-run.sh ...
set -eu

die() {
    echo "long-run.sh: $*" >&2
    exit 2
}

acc= from= count= gpu=auto hours=0 until=0 max_failures=6 wait_seconds=60
while [ $# -gt 0 ]; do
    case $1 in
        --)
            shift
            break
            ;;
        -h | --help)
            sed -n '2,23p' "$0"
            exit 0
            ;;
        --acc | --from | --count | --gpu | --stop-after-hours | --until | --max-failures | --wait-seconds)
            [ $# -ge 2 ] || die "$1 needs a value"
            case $1 in
                --acc) acc=$2 ;;
                --from) from=$2 ;;
                --count) count=$2 ;;
                --gpu) gpu=$2 ;;
                --stop-after-hours) hours=$2 ;;
                --until) until=$2 ;;
                --max-failures) max_failures=$2 ;;
                --wait-seconds) wait_seconds=$2 ;;
            esac
            shift 2
            ;;
        *) die "unknown option $1 (--help lists them; arguments for acc batch go after --)" ;;
    esac
done
[ -n "$acc" ] && [ -n "$from" ] && [ -n "$count" ] || die "--acc, --from and --count are needed (--help shows examples)"
case $gpu in auto | on | off) ;; *) die "--gpu is auto, on or off" ;; esac
# What is left in "$@" goes to acc batch when the run starts.

checkpoint=checkpoints/batch-$from-$count.json
session=
[ "$hours" = 0 ] || session="$session --stop-after $hours"
[ "$until" = 0 ] || session="$session --until $until"

stamp() {
    date '+%Y-%m-%dT%H:%M:%S'
}

done_so_far() {
    if [ -f "$checkpoint" ]; then
        grep -o '"done": *[0-9]*' "$checkpoint" | head -n 1 | grep -o '[0-9]*$' || echo 0
    else
        echo 0
    fi
}

# Ctrl+C reaches acc too, which pauses; this only stops the script from starting it again. (A trap, unlike an ignored
# signal, isn't passed on to acc.)
interrupted=
trap 'interrupted=yes' INT

failures=0
reached=$(done_so_far)
while true; do
    code=0
    if [ -f "$checkpoint" ]; then
        echo "$(stamp) acc batch --resume $checkpoint$session"
        # $session is split into words on purpose: it holds only flags and numbers.
        # shellcheck disable=SC2086
        "$acc" batch --resume "$checkpoint" $session || code=$?
    else
        echo "$(stamp) acc batch --from $from --count $count --gpu $gpu $*$session"
        # shellcheck disable=SC2086
        "$acc" batch --from "$from" --count "$count" --gpu "$gpu" "$@" $session || code=$?
    fi
    now=$(done_so_far)
    case $code in
        0)
            if [ -f "$checkpoint" ]; then
                echo "$(stamp) paused at $now deals. The same command carries on."
            else
                echo "$(stamp) finished: the report is beside where the checkpoint was."
            fi
            exit 0
            ;;
        74)
            echo "$(stamp) out of room on the disk. Make room; the same command carries on."
            exit 74
            ;;
        126 | 127)
            echo "$(stamp) $acc wouldn't start (exit code $code): check the path, and that it is built. Nothing is lost."
            exit 1
            ;;
        130)
            echo "$(stamp) stopped by hand. The same command carries on."
            exit 130
            ;;
        *)
            # 75: the GPU failed, and a new process usually gets it back. Anything else is a crash: the last checkpoint
            # stands. Either way it is worth trying again, unless that keeps getting nowhere.
            if [ -n "$interrupted" ]; then
                echo "$(stamp) stopped by hand, with exit code $code at $now deals. The same command carries on."
                exit "$code"
            fi
            [ "$now" -le "$reached" ] || failures=0
            reached=$now
            failures=$((failures + 1))
            echo "$(stamp) exit code $code at $now deals, try $failures of $max_failures without progress"
            if [ "$failures" -ge "$max_failures" ]; then
                echo "$(stamp) giving up: it isn't getting anywhere. Nothing is lost; see the .events.log."
                exit "$code"
            fi
            sleep "$wait_seconds" || true
            if [ -n "$interrupted" ]; then
                echo "$(stamp) stopped by hand while waiting. The same command carries on."
                exit 130
            fi
            ;;
    esac
done
