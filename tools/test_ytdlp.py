#!/usr/bin/env python3
"""
Test script for yt-dlp with exact production parameters from doradura.
Mirrors the configuration from video.rs and audio.rs.

Usage:
  # Test video download
  python3 test_ytdlp.py video https://youtu.be/jNQXAC9IVRw

  # Test audio download
  python3 test_ytdlp.py audio https://youtu.be/jNQXAC9IVRw

  # Test with custom proxy
  WARP_PROXY=http://100.127.84.48:7777 python3 test_ytdlp.py video https://youtu.be/jNQXAC9IVRw
"""

import os
import sys
import subprocess
import tempfile
from pathlib import Path


def get_proxy():
    """Get WARP_PROXY from environment (matches get_cached_warp_proxy in metadata.rs)."""
    return os.environ.get('WARP_PROXY')


def build_common_args():
    """Common args for both audio and video (matches video.rs:100-131 and audio.rs:128-164)."""
    return [
        "--newline",
        "--force-overwrites",        # Prevent postprocessing conflicts
        "--no-playlist",              # Download single video, not entire playlist
        "--concurrent-fragments", "1",
        "--fragment-retries", "10",
        "--socket-timeout", "30",
        "--http-chunk-size", "2097152",
        "--sleep-requests", "2",
        "--sleep-interval", "3",
        "--max-sleep-interval", "10",
        "--limit-rate", "5M",
        # Exponential backoff for 403/rate-limit errors
        "--retry-sleep", "http:exp=1:30",    # 1s -> 2s -> 4s -> ... up to 30s
        "--retry-sleep", "fragment:exp=1:30", # same for fragment errors
        "--retries", "15",                     # retry main request up to 15 times
    ]


def add_no_cookies_args(args, proxy):
    """
    v5.0 FALLBACK CHAIN: First try WITHOUT cookies (new yt-dlp 2026+ mode)
    Matches add_no_cookies_args from metadata.rs:351-373
    """
    if proxy:
        print(f"[NO_COOKIES] Using proxy: {proxy}")
        args.extend(["--proxy", proxy])
    else:
        print("[NO_COOKIES] No proxy, using direct connection")

    print("[NO_COOKIES] Running WITHOUT cookies and WITHOUT PO Token (modern yt-dlp mode)")


def test_video(url, output_dir):
    """
    Test video download with production parameters.
    Matches video.rs:80-148
    """
    output_path = os.path.join(output_dir, "test_video.mp4")

    args = ["yt-dlp"]
    args.extend(["-o", output_path])
    args.extend(build_common_args())

    # Video-specific args (video.rs:102-105)
    args.extend([
        "--format", "bestvideo[ext=mp4][height<=1080]+bestaudio[ext=m4a]/best[ext=mp4]/best",
        "--merge-output-format", "mp4",
        "--postprocessor-args", "Merger:-movflags +faststart",
    ])

    proxy = get_proxy()
    add_no_cookies_args(args, proxy)

    # v5.0 strategy: android + web_music clients (video.rs:138-144)
    args.extend([
        "--extractor-args", "youtube:player_client=android,web_music;formats=missing_pot",
        "--js-runtimes", "deno",
        "--no-check-certificate",
    ])

    # Note: impersonate removed - not needed for android_vr client (video.rs:146)

    args.append(url)

    print("\n" + "="*80)
    print("VIDEO DOWNLOAD TEST")
    print("="*80)
    print(f"Command: {' '.join(args)}")
    print("="*80 + "\n")

    result = subprocess.run(args)

    if result.returncode == 0 and os.path.exists(output_path):
        file_size = os.path.getsize(output_path)
        print(f"\n✅ SUCCESS: Video downloaded to {output_path} ({file_size:,} bytes)")
        return True
    else:
        print(f"\n❌ FAILED: Exit code {result.returncode}")
        return False


def test_audio(url, output_dir):
    """
    Test audio download with production parameters.
    Matches audio.rs:91-186
    """
    output_path = os.path.join(output_dir, "test_audio.mp3")
    bitrate = "320"

    args = ["yt-dlp"]
    args.extend(["-o", output_path])
    args.extend(build_common_args())

    # Audio-specific args (audio.rs:133-164)
    args.extend([
        "--extract-audio",
        "--audio-format", "mp3",
        "--audio-quality", "0",
        "--add-metadata",
        "--embed-thumbnail",
        "--no-check-certificate",
        "--postprocessor-args", f"ffmpeg:-acodec libmp3lame -b:a {bitrate}k",
    ])

    proxy = get_proxy()
    add_no_cookies_args(args, proxy)

    # v5.0 strategy: android + web_music clients (audio.rs:171-177)
    args.extend([
        "--extractor-args", "youtube:player_client=android,web_music;formats=missing_pot",
        "--js-runtimes", "deno",
    ])

    # Note: impersonate removed - not needed for android_vr client (audio.rs:179)

    args.append(url)

    print("\n" + "="*80)
    print("AUDIO DOWNLOAD TEST")
    print("="*80)
    print(f"Command: {' '.join(args)}")
    print("="*80 + "\n")

    result = subprocess.run(args)

    if result.returncode == 0 and os.path.exists(output_path):
        file_size = os.path.getsize(output_path)
        print(f"\n✅ SUCCESS: Audio downloaded to {output_path} ({file_size:,} bytes)")
        return True
    else:
        print(f"\n❌ FAILED: Exit code {result.returncode}")
        return False


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)

    mode = sys.argv[1].lower()
    url = sys.argv[2]

    if mode not in ['video', 'audio']:
        print("Error: Mode must be 'video' or 'audio'")
        print(__doc__)
        sys.exit(1)

    # Create temp directory for downloads
    with tempfile.TemporaryDirectory() as tmpdir:
        print(f"Output directory: {tmpdir}\n")

        if mode == 'video':
            success = test_video(url, tmpdir)
        else:
            success = test_audio(url, tmpdir)

        sys.exit(0 if success else 1)


if __name__ == '__main__':
    main()
