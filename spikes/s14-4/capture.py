"""S14-4 `web-compare`: one pass over the site list in Firefox headless.

For each site, read-only:
- load its one page;
- wait for the load to finish and the page to settle;
- keep its text in reading order (`document.body.innerText`: the text as
  laid out, hidden parts left out) and one screenshot of the first screen.

Each run has a fresh profile with every cookie refused. There are no logins,
form posts, clicks or scrolling, and the script pauses between sites.

Usage: python3 capture.py SITES OUT
Writes OUT/<name>.txt, OUT/<name>.png and OUT/pages.json (one record a site).
"""

import json
import os
import sys
import time

from selenium import webdriver
from selenium.common.exceptions import WebDriverException
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service

WIDTH, HEIGHT = 1280, 800
LOAD_TIMEOUT_S = 45
SETTLE_S = 3
# Between two sites: one request burst a site, well apart.
POLITE_DELAY_S = 5


def sites(path):
    out = []
    for line in open(path, encoding="utf-8"):
        line = line.split("#", 1)[0].strip()
        if line:
            name, url = line.split(None, 1)
            out.append((name, url.strip()))
    return out


def browser():
    options = Options()
    options.add_argument("-headless")
    options.add_argument(f"--width={WIDTH}")
    options.add_argument(f"--height={HEIGHT}")
    # Refuse every cookie: nothing is kept or sent back (2 = block all).
    options.set_preference("network.cookie.cookieBehavior", 2)
    # Nothing plays, nothing is remembered, nothing is reported.
    options.set_preference("media.autoplay.default", 5)
    options.set_preference("signon.rememberSignons", False)
    options.set_preference("browser.formfill.enable", False)
    options.set_preference("datareporting.policy.dataSubmissionEnabled", False)
    options.set_preference("toolkit.telemetry.enabled", False)
    options.set_preference("geo.enabled", False)
    driver_path = os.environ.get("GECKOWEBDRIVER")
    service = Service(os.path.join(driver_path, "geckodriver")) if driver_path else Service()
    driver = webdriver.Firefox(options=options, service=service)
    driver.set_page_load_timeout(LOAD_TIMEOUT_S)
    driver.set_window_size(WIDTH, HEIGHT)
    return driver


def capture(driver, name, url, out):
    record = {"name": name, "url": url}
    start = time.monotonic()
    try:
        driver.get(url)
        deadline = time.monotonic() + LOAD_TIMEOUT_S
        while driver.execute_script("return document.readyState") != "complete":
            if time.monotonic() > deadline:
                raise WebDriverException("the page never finished loading")
            time.sleep(0.25)
        time.sleep(SETTLE_S)
        text = driver.execute_script("return document.body ? document.body.innerText : ''") or ""
        record.update(
            status="ok",
            final_url=driver.current_url,
            title=driver.title,
            chars=len(text),
            words=len(text.split()),
        )
        with open(os.path.join(out, f"{name}.txt"), "w", encoding="utf-8") as f:
            f.write(text)
        driver.save_screenshot(os.path.join(out, f"{name}.png"))
    except WebDriverException as e:
        record.update(status="error", error=(e.msg or str(e)).splitlines()[0][:300])
    record["seconds"] = round(time.monotonic() - start, 2)
    return record


def main(sites_path, out):
    os.makedirs(out, exist_ok=True)
    records = []
    driver = browser()
    try:
        for n, (name, url) in enumerate(sites(sites_path)):
            if n:
                time.sleep(POLITE_DELAY_S)
            record = capture(driver, name, url, out)
            records.append(record)
            print(f"{name}: {record['status']} {record.get('words', '')} words in {record['seconds']} s", flush=True)
        records.append({"firefox": driver.capabilities.get("browserVersion")})
    finally:
        driver.quit()
    with open(os.path.join(out, "pages.json"), "w", encoding="utf-8") as f:
        json.dump(records, f, indent=2)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
