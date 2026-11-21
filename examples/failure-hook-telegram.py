#!/usr/bin/env python3
"""Set the variables below, chmod +x this file, and set failure_hook to its path."""

import json
import sys
import time
from datetime import datetime
from html import escape
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


TELEGRAM_BASE_URL = "https://api.telegram.org"
BOT_TOKEN = ""  # BotFather token; keep this file private after configuring it.
CHAT_IDS = []  # Recipient user IDs, e.g. [123456789].
ALLOWED_TRIGGERS = {"cron", "manual", "startup"}
SEND_OUTPUT = True


def main():
    event = json.load(sys.stdin)
    if event["trigger"] not in ALLOWED_TRIGGERS:
        return
    if not BOT_TOKEN or not CHAT_IDS:
        raise ValueError("Configure BOT_TOKEN and CHAT_IDS first")
    if not TELEGRAM_BASE_URL.startswith("https://"):
        raise ValueError("TELEGRAM_BASE_URL must use HTTPS")

    ended_at = datetime.fromisoformat(event["ended_at"].replace("Z", "+00:00"))
    ended_at = ended_at.astimezone().strftime("%Y/%m/%d %H:%M")
    detail = (
        event.get("output") or "(no output)"
        if SEND_OUTPUT
        else event["error"]
    )
    body = (
        f"Job: {event['job_name']}\n"
        f"Trigger: {event['trigger']}\n"
        f"Time: {ended_at}\n"
        f"Attempts: {event['attempt']}\n\n"
    )
    # 1700 code points leave room for metadata within Telegram's 4096-unit limit.
    pages = [detail[start : start + 1700] for start in range(0, len(detail), 1700)]
    endpoint = f"{TELEGRAM_BASE_URL.rstrip('/')}/bot{BOT_TOKEN}/sendMessage"
    for chat_id in CHAT_IDS:
        for number, page in enumerate(pages, 1):
            if number > 1:
                time.sleep(1)
            text = "#FlashCron\n\n"
            if len(pages) > 1:
                text += f"[{number}/{len(pages)}]\n"
            text += escape(body)
            text += (
                f"Output:\n<pre>{escape(page)}</pre>"
                if SEND_OUTPUT
                else f"Error: {escape(page)}"
            )
            request = Request(
                endpoint,
                data=json.dumps({"chat_id": chat_id, "text": text, "parse_mode": "HTML"}).encode("utf-8"),
                headers={
                    "Content-Type": "application/json",
                    "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/158.0.0.0 Safari/537.36",
                },
                method="POST",
            )
            try:
                with urlopen(request, timeout=30) as response:
                    result = json.load(response)
            except HTTPError as error:
                try:
                    description = json.load(error).get("description", "Request rejected")
                except (ValueError, UnicodeError):
                    description = "Non-JSON error response"
                raise RuntimeError(f"Telegram HTTP {error.code}: {description}") from None
            except URLError:
                raise RuntimeError("Telegram connection failed") from None
            if not result.get("ok"):
                raise RuntimeError("Telegram rejected the notification")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, RuntimeError) as error:
        print(f"Telegram notification failed: {error}", file=sys.stderr)
        sys.exit(1)
