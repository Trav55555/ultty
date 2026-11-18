"""Independent native mouse/clipboard/link gates over actual Porygon history."""
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import threading
import time
import unicodedata

import pytest
from PIL import Image, ImageGrab
from daemon_support import Daemon, ConversationStore
from porygon.events import TokenEvent
from terminal_support import Terminal, wait, run as grid_regression

pytest_plugins = ["conftest"]

UNICODE_LINE = "UNI café 雪 e\u0301 🙂 END"
UNICODE_PART = "café 雪 e\u0301 🙂"
CODE_LINES = ["  alpha  β雪  tail  ", "    next    café  "]
TAB_RAW = "\tTAB  雪  "
TAB_RENDERED = "        TAB  雪  "
URL = "https://qualification.invalid/native?x=1&y=two"
WRAPPED = "WRAP-" + "0123456789 abcdefghijklmnopqrstuvwxyz " * 5 + "-END"
BODY = UNICODE_LINE + "\n\n```text\n" + "\n".join(CODE_LINES + [TAB_RAW]) + "\n```\n\nLINK " + URL + "\n\n" + WRAPPED
SCROLL_LINES = [f"row-{i:03d}  café 雪  tail  " for i in range(100)]
STREAM_LINE = "REVISION café 雪 e\u0301 🙂 KEEP"


class MutableSession:
    def __init__(self):
        self.mutate = threading.Event()
        self.messages = []

    async def process_request_async(self, message, **kwargs):
        self.messages.append(message)
        yield TokenEvent(text=STREAM_LINE)
        cancel = kwargs["cancel_event"]
        while not self.mutate.is_set() and not cancel.is_set():
            await asyncio.sleep(.02)
        if not cancel.is_set():
            yield TokenEvent(text=" CHANGED")
        while not cancel.is_set():
            await asyncio.sleep(.02)


def pixel_difference(first, second, box):
    with Image.open(first) as a, Image.open(second) as b:
        a, b = a.convert("RGB"), b.convert("RGB")
        left, top, right, bottom = box
        return sum(a.getpixel((x,y)) != b.getpixel((x,y)) for y in range(top,bottom) for x in range(left,right))


def regions(value):
    if isinstance(value, dict):
        if "text" in value and "x" in value:
            yield value
        for child in value.values():
            yield from regions(child)
    elif isinstance(value, list):
        for child in value:
            yield from regions(child)


def cells(text):
    return sum(0 if unicodedata.combining(char) else 2 if unicodedata.east_asian_width(char) in "WF" else 1 for char in text)


class Driver:
    def __init__(self, term, binary):
        self.term = term
        self.width, self.height = 1000, 650
        command = "i=0; while [ \"$i\" -lt 80 ]; do printf 'GRID_FALLBACK_MUST_NOT_COPY________________________\\n'; i=$((i+1)); done; exec " + shlex.quote(str(binary))
        opened = json.loads(term.ctl("--json", "new-tab", "/bin/sh", "-c", command))
        self.app_tab, self.app_pane = opened["tab_index"] + 1, opened["pane"]
        self.cases = []
        self.mouse_events = []
        wait(lambda: any("Message Porygon" in r["text"] or r.get("role") == "input" for r in self.snapshot("startup")), "native composer")

    def snapshot(self, name):
        document = json.loads(self.term.ctl("--json", "regions"))
        (self.term.out / f"{name}.regions.json").write_text(json.dumps(document, indent=2))
        return list(regions(document))

    def row(self, text, name="locate"):
        return wait(lambda: next((r for r in self.snapshot(name) if r["text"] == text), None), f"visible native text {text!r}")

    def pixel(self, row, column):
        info = json.loads(self.term.ctl("--json", "info"))
        # Ultty reserves the outer8px for CSD resize. Stay inside display cell0
        # while avoiding the resize hit band; selection columns are floored.
        column = .75 if column == 0 else column
        # The fixture always has at least two tabs; native content begins below
        # the 48px window chrome and 24px tab strip. Screenshots retain calibration.
        return (round((row["x"] + column) * self.width / info["cols"]),
                round(72 + (row["y"] + .5) * (self.height - 72) / info["rows"]))

    def screenshot(self, name):
        geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", self.term.window], env=self.term.env, text=True)
        values = dict(line.split("=",1) for line in geometry.splitlines() if "=" in line)
        x,y,width,height = (int(values[key]) for key in ("X","Y","WIDTH","HEIGHT"))
        time.sleep(.1)
        ImageGrab.grab(xdisplay=self.term.env["DISPLAY"], bbox=(x,y,x+width,y+height)).save(self.term.out / f"{name}.png")
        self.term.ctl("screenshot", self.term.out / f"{name}.control.png")

    def mouse(self, *args):
        self.mouse_events.append({"monotonic": time.monotonic(), "args": list(args)})
        (self.term.out / "mouse-events.json").write_text(json.dumps(self.mouse_events, indent=2))
        subprocess.run(["xdotool", "windowfocus", "--sync", self.term.window, *map(str, args)], env=self.term.env, check=True, timeout=15)

    def drag(self, start, end, ctrl=False, release=True):
        if ctrl:
            self.mouse("keydown", "ctrl")
        self.mouse("mousemove", "--window", self.term.window, *start, "mousedown", "1")
        time.sleep(.1)
        self.mouse("mousemove", "--window", self.term.window, *end)
        time.sleep(.15)
        if release:
            self.mouse("mouseup", "1")
            if ctrl:
                self.mouse("keyup", "ctrl")

    def clear_completed_selection(self, row):
        # Snapshot retention is intentional. Pixel baselines need a genuine
        # user click to clear it before measuring a new held drag.
        self.mouse("mousemove","--window",self.term.window,*self.pixel(row,12),"click","1")
        time.sleep(.1)

    def open_conversation(self, title):
        self.term.ctl("send-key", "f2")
        wait(lambda: any(r["text"].startswith("Search:") for r in self.snapshot("picker-ready")), "conversation picker ready")
        self.type_ascii(title)
        wait(lambda: any(r["text"] == "Search: " + title for r in self.snapshot("picker-query")), "exact conversation query")
        self.term.ctl("send-key", "enter")
        time.sleep(.3)
        self.term.ctl("send-key", "home", "ctrl")

    def type_ascii(self, text):
        for char in text:
            self.term.ctl("send-key", char)

    def clipboard(self, name):
        capture = self.term.out / f"{name}.clipboard.txt"
        self.term.ctl("new-tab", "/bin/sh", "-c", f"cat > {shlex.quote(str(capture))}; printf '\\nCAPTURE_DONE\\n'; sleep 120")
        time.sleep(.2)
        self.term.key("ctrl+shift+v")
        self.term.ctl("send-key", "d", "ctrl")
        self.term.ctl("send-key", "d", "ctrl")
        wait(lambda: "CAPTURE_DONE" in self.term.ctl("text"), "system clipboard capture")
        result = capture.read_text()
        self.term.ctl("switch-tab", self.app_tab)
        return result

    def seed_clipboard(self, name):
        value = "sentinel-" + name
        encoded = base64.b64encode(value.encode()).decode()
        self.term.ctl("new-tab", "/bin/sh", "-c", f"printf '\\033]52;c;{encoded}\\007'; printf 'CLIP_SEEDED\\n'; sleep 120")
        wait(lambda: "CLIP_SEEDED" in self.term.ctl("text"), "fresh clipboard sentinel")
        self.term.ctl("switch-tab", self.app_tab)
        time.sleep(.1)
        return value

    def copy_case(self, name, expected, action):
        self.seed_clipboard(name)
        action()
        self.screenshot(name)
        grid_selection = self.term.ctl("selection")
        self.term.key("ctrl+shift+c")
        actual = self.clipboard(name)
        record = {"case": name, "expected": expected, "actual": actual,
                  "pass": actual == expected, "selection_query": grid_selection}
        self.cases.append(record)
        self.save()

    def save(self):
        (self.term.out / "native-interaction-receipt.json").write_text(json.dumps({"cases": self.cases, "pixel_capture": "actual X11 ImageGrab; control screenshots retained separately", "human_task_trial": False}, ensure_ascii=False, indent=2))


@pytest.mark.timeout(180)
@pytest.mark.asyncio
async def test_native_selection_links_wrap_resize_scroll(mock_context, mock_database, monkeypatch):
    session = MutableSession()
    monkeypatch.setattr(mock_context.user, "_session", session)
    store = ConversationStore()
    for title, body in [("selection-case", BODY), ("selection-scroll", "```text\n" + "\n".join(SCROLL_LINES) + "\n```")]:
        await store.create_conversation(title=title, conv_id=title)
        await store.add_message(title, "assistant", body)
    binary = Path(os.environ["PORYGON_NATIVE_TEST_BINARY"]).resolve()
    with Daemon(mock_context, monkeypatch) as daemon, Terminal(daemon.env) as term:
        driver = Driver(term, binary)
        (term.out / "binary-fixture-hashes.json").write_text(json.dumps({str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in [binary, Path(os.environ["ULTTY_BINARY"]), Path(os.environ["ULTTYCTL_BINARY"]), Path(__file__)]}, indent=2))
        term.ctl("resize","800","480")
        driver.width,driver.height=800,480
        term.ctl("send-key","f1")
        def complete_help():
            rows=driver.snapshot("compact-help")
            required=("Review: c copies evidence.","Ctrl+Shift+C: copy selection.")
            found={text:next((r for r in rows if text in r["text"]),None) for text in required}
            return found if all(found.values()) else None
        help_rows=wait(complete_help,"complete compact evidence and selection hints")
        assert all(r["width"]>=cells(text) for text,r in help_rows.items()),help_rows
        driver.screenshot("help-800x480")
        (term.out/"compact-help-geometry.json").write_text(json.dumps(help_rows,indent=2))
        term.ctl("send-key","escape")
        term.ctl("resize","1000","650")
        driver.width,driver.height=1000,650
        driver.open_conversation("selection-case")
        unicode_row = driver.row(UNICODE_LINE)
        driver.copy_case("unicode-partial", UNICODE_PART, lambda: driver.drag(driver.pixel(unicode_row, 4), driver.pixel(unicode_row, cells("UNI " + UNICODE_PART))))
        first, second = driver.row(CODE_LINES[0]), driver.row(CODE_LINES[1])
        driver.copy_case("code-multiline-spaces", "\n".join(CODE_LINES), lambda: driver.drag(driver.pixel(first, 0), driver.pixel(second, cells(CODE_LINES[1]))))
        tab_row = driver.row(TAB_RENDERED)
        driver.copy_case("code-tab-rendered-spaces", TAB_RENDERED, lambda: driver.drag(driver.pixel(tab_row, 0), driver.pixel(tab_row, cells(TAB_RENDERED))))

        term.ctl("resize", "800", "480")
        driver.width, driver.height = 800, 480
        term.ctl("send-key", "end", "ctrl")
        wrapped = wait(lambda: next((r for r in driver.snapshot("wrapped") if r["text"].startswith("WRAP-")), None), "wrapped paragraph start")
        visible = driver.snapshot("wrapped-rows")
        next_row = next(r for r in visible if r["y"] == wrapped["y"] + 1 and r["height"] == 1 and r["text"])
        split = len(wrapped["text"])
        assert WRAPPED.startswith(wrapped["text"] + next_row["text"]), "fixture must match known rendered paragraph"
        expected_wrapped = WRAPPED[5:split + 8]
        driver.copy_case("wrapped-soft-break", expected_wrapped, lambda: driver.drag(driver.pixel(wrapped, 5), driver.pixel(next_row, 8)))
        def select_then_resize():
            driver.drag(driver.pixel(wrapped, 5), driver.pixel(next_row, 8))
            term.ctl("resize", "1000", "650")
            driver.width, driver.height = 1000, 650
            time.sleep(.3)
        driver.copy_case("selection-persists-resize", expected_wrapped, select_then_resize)

        term.ctl("send-key", "home", "ctrl")
        link = wait(lambda: next((r for r in driver.snapshot("link") if URL in r["text"]), None), "visible native URL")
        prefix = link["text"].index(URL)
        link_point = driver.pixel(link, cells(link["text"][:prefix]) + 8)
        opener = term.out / "opened-links.txt"
        driver.mouse("mousemove", "--window", term.window, *link_point, "keydown", "ctrl", "click", "1", "keyup", "ctrl")
        time.sleep(.4)
        opened = opener.read_text().splitlines() if opener.exists() else []
        driver.cases.append({"case": "native-link-exact-target", "expected": [URL], "actual": opened, "pass": opened == [URL]})
        before = list(opened)
        driver.drag(link_point, (link_point[0] + 70, link_point[1]))
        ordinary_after = opener.read_text().splitlines() if opener.exists() else []
        driver.cases.append({"case": "ordinary-drag-does-not-open", "expected": before, "actual": ordinary_after, "pass": ordinary_after == before, "positive_click_control_passed": opened == [URL]})
        driver.drag(link_point, (link_point[0] + 70, link_point[1]), ctrl=True)
        after = opener.read_text().splitlines() if opener.exists() else []
        driver.cases.append({"case": "ctrl-drag-does-not-open", "expected": before, "actual": after, "pass": after == before, "positive_click_control_passed": opened == [URL]})
        driver.save()
        driver.screenshot("native-link")

        driver.open_conversation("selection-scroll")
        def completed_selection_then_scroll():
            term.ctl("send-key", "home", "ctrl")
            first_row, second_row = driver.row(SCROLL_LINES[0]), driver.row(SCROLL_LINES[1])
            driver.drag(driver.pixel(first_row, 0), driver.pixel(second_row, cells(SCROLL_LINES[1])))
            term.ctl("send-key", "end", "ctrl")
            driver.row(SCROLL_LINES[-1])
        driver.copy_case("completed-selection-persists-scroll", "\n".join(SCROLL_LINES[:2]), completed_selection_then_scroll)
        term.ctl("send-key", "home", "ctrl")
        initial = driver.row(SCROLL_LINES[0])
        def select_across_scroll():
            driver.drag(driver.pixel(initial, 0), driver.pixel(initial, 8), release=False)
            driver.screenshot("active-scroll-before")
            driver.mouse("click", "--repeat", "3", "--delay", "80", "5")
            time.sleep(.4)
            rows = [r for r in driver.snapshot("active-drag-scrolled") if r["text"].startswith("row-")]
            assert rows and min(int(r["text"][4:7]) for r in rows) > 0, "wheel must actually advance transcript during held drag"
            last = max(rows, key=lambda r: r["y"])
            index = int(last["text"][4:7])
            point = driver.pixel(last, cells(last["text"]))
            driver.mouse("mousemove", "--window", term.window, *point, "mouseup", "1")
            return "\n".join(SCROLL_LINES[:index+1])
        sentinel = driver.seed_clipboard("active-drag-scroll")
        initial = driver.row(SCROLL_LINES[0])
        driver.clear_completed_selection(initial)
        driver.screenshot("active-scroll-unselected")
        expected_scroll = select_across_scroll()
        driver.screenshot("active-scroll-after")
        scroll_selection = term.ctl("selection")
        term.key("ctrl+shift+c")
        actual_scroll = driver.clipboard("active-drag-scroll")
        exact = actual_scroll == expected_scroll
        info = json.loads(term.ctl("--json", "info"))
        left,middle = driver.pixel(initial, 2)
        right,_ = driver.pixel(initial, 7)
        half_height = (driver.height-72) / info["rows"] / 2
        scroll_box = [left, round(middle-half_height+2), right, round(middle+half_height-2)]
        active_scroll_pixels = pixel_difference(term.out / "active-scroll-unselected.png", term.out / "active-scroll-before.png", scroll_box)
        safe_cancel = actual_scroll == sentinel and "no selection" in scroll_selection.lower() and active_scroll_pixels > 30
        driver.cases.append({"case": "active-drag-scroll", "expected_complete": expected_scroll, "expected_cancel": sentinel,
                             "actual": actual_scroll, "selection_query": scroll_selection, "pass": exact or safe_cancel,
                             "active_highlight_changed_pixels": active_scroll_pixels,
                             "outcome": "complete" if exact else "safe-cancel-pending-pixel-review" if safe_cancel else "unsafe-or-unsupported"})
        driver.save()
        driver.screenshot("after-scroll-selection")

        # A single viewport jump removes the unencountered middle rows. This is
        # the explicit bounded-retention cancellation case, not auto-scroll.
        term.ctl("send-key","home","ctrl")
        gap_row=driver.row(SCROLL_LINES[0])
        gap_sentinel=driver.seed_clipboard("missing-rows")
        gap_row=driver.row(SCROLL_LINES[0])
        driver.clear_completed_selection(gap_row)
        driver.screenshot("gap-unselected")
        driver.drag(driver.pixel(gap_row,0),driver.pixel(gap_row,8),release=False)
        driver.screenshot("gap-selected")
        term.ctl("send-key","end","ctrl")
        last_row=driver.row(SCROLL_LINES[-1])
        driver.screenshot("gap-invalidated")
        driver.mouse("mousemove","--window",term.window,*driver.pixel(last_row,8),"mouseup","1")
        term.key("ctrl+shift+c")
        gap_actual=driver.clipboard("missing-rows")
        gap_left,gap_middle=driver.pixel(gap_row,2)
        gap_right,_=driver.pixel(gap_row,7)
        gap_box=[gap_left,round(gap_middle-half_height+2),gap_right,round(gap_middle+half_height-2)]
        gap_active_pixels=pixel_difference(term.out/"gap-unselected.png",term.out/"gap-selected.png",gap_box)
        driver.cases.append({"case":"missing-rows-cancel-without-partial-or-grid-fallback","expected":gap_sentinel,"actual":gap_actual,
                             "active_highlight_changed_pixels":gap_active_pixels,"pixel_review_required":True,
                             "pass":gap_actual==gap_sentinel and gap_active_pixels>30})
        driver.save()

        # A real synthetic stream changes producer revision while the pointer
        # is held. The selected prefix is unchanged, so its pixel crop isolates
        # highlight removal from the appended source text.
        term.ctl("send-key", "f2")
        term.ctl("send-key", "n", "ctrl")
        driver.type_ascii("revision-probe")
        term.ctl("send-key", "enter")
        streaming = driver.row(STREAM_LINE, "revision-source-before")
        sentinel = driver.seed_clipboard("revision-change")
        streaming=driver.row(STREAM_LINE,"revision-source-ready")
        driver.clear_completed_selection(streaming)
        driver.screenshot("revision-unselected")
        driver.drag(driver.pixel(streaming, 1), driver.pixel(streaming, 15), release=False)
        driver.screenshot("revision-selected")
        session.mutate.set()
        driver.row(STREAM_LINE + " CHANGED", "revision-source-after")
        driver.screenshot("revision-invalidated")
        driver.mouse("mouseup", "1")
        term.key("ctrl+shift+c")
        revision_selection = term.ctl("selection")
        actual_revision = driver.clipboard("revision-change")
        info = json.loads(term.ctl("--json", "info"))
        left, middle = driver.pixel(streaming, 3)
        right, _ = driver.pixel(streaming, 12)
        half_height = (driver.height - 72) / info["rows"] / 2
        box = [left, round(middle-half_height+2), right, round(middle+half_height-2)]
        active_pixels = pixel_difference(term.out / "revision-unselected.png", term.out / "revision-selected.png", box)
        remaining_pixels = pixel_difference(term.out / "revision-unselected.png", term.out / "revision-invalidated.png", box)
        driver.cases.append({"case": "revision-change-cancels-without-fallback", "expected": sentinel, "actual": actual_revision,
                             "selection_query": revision_selection, "pixel_crop": box, "active_highlight_changed_pixels": active_pixels,
                             "remaining_changed_pixels": remaining_pixels,
                             "pass": actual_revision == sentinel and "no selection" in revision_selection.lower() and active_pixels > 30 and remaining_pixels <= max(5, active_pixels * .05)})
        term.ctl("send-key", "c", "ctrl")
        assert session.messages == ["revision-probe"]
        driver.save()
        print(json.dumps({"artifact_dir": str(term.out), "cases": [{"case": c["case"], "pass": c["pass"]} for c in driver.cases]}))
        assert all(c["pass"] for c in driver.cases), driver.cases


@pytest.mark.timeout(90)
@pytest.mark.asyncio
async def test_native_painted_unicode_anchor_alignment(mock_context, mock_database, monkeypatch):
    reference="A   A   A"
    probes=[("wide","雪",2),("combining","e\u0301",1),("zwj","👩\u200d💻",2)]
    body="```text\n"+reference+"\n"+reference+"\n"+"\n".join(cluster+"A" for _,cluster,_ in probes)+"\n```"
    await ConversationStore().create_conversation(title="paint-case",conv_id="paint-case")
    await ConversationStore().add_message("paint-case","assistant",body)
    binary=Path(os.environ["PORYGON_NATIVE_TEST_BINARY"]).resolve()
    with Daemon(mock_context,monkeypatch) as daemon,Terminal(daemon.env) as term:
        driver=Driver(term,binary)
        driver.open_conversation("paint-case")
        wait(lambda:len([r for r in driver.snapshot("paint-rows") if r["text"]==reference])==2,"two painted ASCII calibration rows")
        driver.screenshot("paint-reference")
        with Image.open(term.out/"paint-reference.png") as image:
            image=image.convert("RGB")
            background=image.getpixel((image.width-30,image.height//2))
            def ink(x,y):
                color=image.getpixel((x,y))
                return max(color)-min(color)<50 and sum(abs(a-b) for a,b in zip(color,background))>80
            # Locate the two repeated ASCII rows in the actual image. Colored
            # timestamp/header ink is excluded; no metadata y/content-top is used.
            candidates=[]
            for y in range(image.height):
                groups=[]
                for x in range(image.width-30):
                    if ink(x,y):
                        if not groups or x>groups[-1][-1]+1: groups.append([x])
                        else: groups[-1].append(x)
                groups=[group for group in groups if len(group)>=3]
                if len(groups)==3:
                    starts=[group[0] for group in groups]
                    first_gap,second_gap=starts[1]-starts[0],starts[2]-starts[1]
                    if 32<first_gap<96 and abs(first_gap-second_gap)<2:
                        candidates.append((sum(map(len,groups)),y,starts))
            assert candidates,"painted ASCII reference not located"
            strongest=max(item[0] for item in candidates)
            strong=sorted((y,starts) for strength,y,starts in candidates if strength>=strongest-1)
            bands=[]
            for y,starts in strong:
                if not bands or y>bands[-1][-1][0]+2: bands.append([(y,starts)])
                else: bands[-1].append((y,starts))
            assert len(bands)==2,f"expected two actual ASCII reference rows: {bands}"
            crossbar,starts=bands[0][0]
            row_pitch=bands[1][0][0]-crossbar
            pitch=(starts[2]-starts[0])/8
            assert 8<pitch<24 and 8<row_pitch<30
            template_left=starts[1]
            template_width=round(pitch)
            top,bottom=crossbar,crossbar
            while top>0 and any(ink(x,top-1) for x in range(template_left,template_left+template_width)): top-=1
            while bottom+1<image.height and any(ink(x,bottom+1) for x in range(template_left,template_left+template_width)): bottom+=1
            bottom+=1
            template=[[ink(template_left+x,y) for x in range(template_width)] for y in range(top,bottom)]
            def template_error(left,row_top):
                return sum(ink(left+x,row_top+y)!=template[y][x] for y in range(len(template)) for x in range(template_width))
            error_limit=max(6,template_error(starts[0],top),template_error(starts[2],top))+3
            first_ink=starts[0]
        calibration={"ascii_glyph_starts":starts,"painted_cell_pitch":pitch,"painted_row_pitch":row_pitch,
                     "reference_band":[top,bottom],"glyph_error_limit":error_limit,"source":"actual X11 grayscale ASCII glyph pattern"}
        (term.out/"paint-calibration.json").write_text(json.dumps(calibration,indent=2))
        for ordinal,(name,cluster,expected_cells) in enumerate(probes,start=2):
            driver.row(cluster+"A")
            row_top=top+ordinal*row_pitch
            middle=round(row_top+(bottom-top-1)/2)
            with Image.open(term.out/"paint-reference.png") as image:
                image=image.convert("RGB")
                scores=[]
                for left in range(round(8*pitch)):
                    def target_ink(x,y):
                        color=image.getpixel((x,y))
                        return max(color)-min(color)<50 and sum(abs(a-b) for a,b in zip(color,background))>80
                    error=sum(target_ink(left+x,row_top+y)!=template[y][x] for y in range(len(template)) for x in range(template_width))
                    scores.append((error,left))
            error,painted_left=min(scores)
            expected_left=first_ink+expected_cells*pitch
            driver.seed_clipboard("paint-"+name)
            # Click the image-matched visible glyph, not the metadata column.
            start=(round(painted_left+.25*pitch),middle)
            end=(round(painted_left+1.25*pitch),middle)
            driver.drag(start,end)
            driver.screenshot("paint-"+name+"-selected")
            term.key("ctrl+shift+c")
            actual=driver.clipboard("paint-"+name)
            driver.cases.append({"case":"painted-A-after-"+name,"expected":"A","actual":actual,
                                 "painted_A_ink_x":painted_left,"expected_display_cell_ink_x":expected_left,
                                 "glyph_template_error_pixels":error,"actual_drag_pixels":[start,end],
                                 "pass":actual=="A" and abs(painted_left-expected_left)<=1 and error<=error_limit})
            driver.save()
        print(json.dumps({"artifact_dir":str(term.out),"calibration":calibration,"cases":driver.cases},ensure_ascii=False))
        assert all(c["pass"] for c in driver.cases),driver.cases


@pytest.mark.timeout(90)
@pytest.mark.asyncio
async def test_native_split_pane_selection_isolation(mock_context, mock_database, monkeypatch):
    left_line, right_line = "  LEFT café 雪  A  ", "    RIGHT β雪  B  "
    for title,line in (("pane-a",left_line),("pane-b",right_line)):
        await ConversationStore().create_conversation(title=title, conv_id=title)
        await ConversationStore().add_message(title,"assistant","```text\n"+line+"\n```")
    binary = Path(os.environ["PORYGON_NATIVE_TEST_BINARY"]).resolve()
    with Daemon(mock_context, monkeypatch) as daemon, Terminal(daemon.env) as term:
        driver = Driver(term,binary)
        driver.open_conversation("pane-a")
        command = "i=0; while [ \"$i\" -lt 80 ]; do printf 'GRID_FALLBACK_MUST_NOT_COPY________________________\\n'; i=$((i+1)); done; exec " + shlex.quote(str(binary))
        right_pane = json.loads(term.ctl("--json","--pane",driver.app_pane,"split","right","/bin/sh","-c",command))["pane"]

        def pane_rows(pane):
            document=json.loads(term.ctl("--json","--pane",pane,"regions"))
            return list(regions(document))

        def pane_row(pane,text):
            return wait(lambda: next((r for r in pane_rows(pane) if r["text"]==text),None),f"pane{pane} text {text!r}")

        wait(lambda:any(r.get("role")=="input" for r in pane_rows(right_pane)),"second native pane composer")
        term.ctl("--pane",right_pane,"send-key","f2")
        wait(lambda:any(r["text"].startswith("Search:") for r in pane_rows(right_pane)),"right picker")
        for char in "pane-b":
            term.ctl("--pane",right_pane,"send-key",char)
        term.ctl("--pane",right_pane,"send-key","enter")
        left=pane_row(driver.app_pane,left_line)
        right=pane_row(right_pane,right_line)

        def point(pane,row,column):
            info=json.loads(term.ctl("--json","panes"))
            info=next(p for p in info["panes"] if p["id"]==pane)
            offset=0 if pane==driver.app_pane else driver.width/2
            column=.75 if column==0 else column
            return (round(offset+(row["x"]+column)*(driver.width/2)/info["cols"]),
                    round(72+(row["y"]+.5)*(driver.height-72)/info["rows"]))

        left_prefix=cells(left_line[:left_line.index("café")])
        left_expected="café 雪"
        right_prefix=cells(right_line[:right_line.index("β")])
        right_expected="β雪  B"
        driver.seed_clipboard("split-left")
        driver.screenshot("split-unselected")
        driver.drag(point(driver.app_pane,left,left_prefix),point(driver.app_pane,left,left_prefix+cells(left_expected)))
        driver.screenshot("split-left-selected")
        term.key("ctrl+shift+c")
        actual=driver.clipboard("split-left")
        left_start=point(driver.app_pane,left,left_prefix)
        left_end=point(driver.app_pane,left,left_prefix+cells(left_expected))
        right_start=point(right_pane,right,right_prefix)
        right_end=point(right_pane,right,right_prefix+cells(right_expected))
        left_box=[left_start[0],left_start[1]-4,left_end[0],left_start[1]+4]
        right_box=[right_start[0],right_start[1]-4,right_end[0],right_start[1]+4]
        left_pixels=pixel_difference(term.out/"split-unselected.png",term.out/"split-left-selected.png",left_box)
        right_pixels=pixel_difference(term.out/"split-unselected.png",term.out/"split-left-selected.png",right_box)
        driver.cases.append({"case":"split-left-copy-highlight-confined","expected":left_expected,"actual":actual,
                             "left_changed_pixels":left_pixels,"right_changed_pixels":right_pixels,
                             "pass":actual==left_expected and left_pixels>30 and right_pixels<=5})
        driver.seed_clipboard("split-right")
        driver.drag(point(right_pane,right,right_prefix),point(right_pane,right,right_prefix+cells(right_expected)))
        term.key("ctrl+shift+c")
        actual=driver.clipboard("split-right")
        driver.cases.append({"case":"split-right-copy","expected":right_expected,"actual":actual,"pass":actual==right_expected})
        sentinel=driver.seed_clipboard("split-cross-pane")
        driver.drag(point(driver.app_pane,left,left_prefix),point(right_pane,right,right_prefix+cells(right_expected)))
        driver.screenshot("split-cross-pane")
        term.key("ctrl+shift+c")
        actual=driver.clipboard("split-cross-pane")
        allowed=[left_line[left_line.index("café"):],sentinel]
        driver.cases.append({"case":"cross-pane-drag-never-copies-other-pane","allowed":allowed,"actual":actual,"pass":actual in allowed})
        driver.seed_clipboard("split-resize")
        driver.drag(point(driver.app_pane,left,left_prefix),point(driver.app_pane,left,left_prefix+cells(left_expected)))
        term.ctl("resize","1200","650")
        driver.width=1200
        time.sleep(.3)
        term.key("ctrl+shift+c")
        actual=driver.clipboard("split-resize")
        driver.cases.append({"case":"split-completed-snapshot-resize","expected":left_expected,"actual":actual,"pass":actual==left_expected})
        driver.save()
        driver.screenshot("split-final")
        print(json.dumps({"artifact_dir":str(term.out),"cases":driver.cases},ensure_ascii=False))
        assert all(c["pass"] for c in driver.cases),driver.cases


@pytest.mark.timeout(90)
@pytest.mark.asyncio
async def test_native_semantic_link_labels(mock_context, mock_database, monkeypatch):
    plain_url="https://qualification.invalid/plain#anchor"
    mixed_url="https://qualification.invalid/mixed?a=1&b=two"
    wrapped_url="https://qualification.invalid/wrapped?line=2"
    wrapped_label="wrapped-"+"abcdefghijklmnopqrstuvwxyz "*5
    mixed_label="**BOLD 雪A "+("style "*12).rstrip()+"** and `code e\u0301A` tail"
    body=(f"PLAIN [SOURCE]({plain_url}) outside\n\nMIX [{mixed_label}]({mixed_url})\n\n"
          f"WRAP [{wrapped_label}]({wrapped_url})\n\n[UNSAFE](javascript:alert(1))")
    await ConversationStore().create_conversation(title="links-case",conv_id="links-case")
    await ConversationStore().add_message("links-case","assistant",body)
    binary=Path(os.environ["PORYGON_NATIVE_TEST_BINARY"]).resolve()
    with Daemon(mock_context,monkeypatch) as daemon,Terminal(daemon.env) as term:
        driver=Driver(term,binary)
        driver.open_conversation("links-case")
        opener=term.out/"opened-links.txt"
        expected=[]
        def opened():
            return opener.read_text().splitlines() if opener.exists() else []
        def containing(text,name):
            return wait(lambda:next((r for r in driver.snapshot(name) if text in r["text"]),None),f"label text {text!r}")
        def click_case(name,row,column,target):
            point=driver.pixel(row,column)
            driver.mouse("mousemove","--window",term.window,*point,"keydown","ctrl","click","1","keyup","ctrl")
            if target is not None: expected.append(target)
            time.sleep(.3)
            actual=opened()
            driver.cases.append({"case":name,"expected":list(expected),"actual":actual,"clicked_row":row["text"],"column":column,"pass":actual==expected})
            driver.save()

        plain=containing("PLAIN SOURCE","plain-label")
        click_case("plain-semantic-label",plain,cells(plain["text"][:plain["text"].index("SOURCE")])+2,plain_url)
        click_case("nonlink-prefix-does-not-open",plain,2,None)
        bold=containing("BOLD","mixed-bold")
        assert "**" not in bold["text"],"fixture bold delimiter must be parsed, not literal text"
        click_case("mixed-bold-semantic-label",bold,cells(bold["text"][:bold["text"].index("BOLD")])+2,mixed_url)
        code=containing("code e\u0301A","mixed-code")
        assert "`" not in code["text"],"fixture inline code must be parsed"
        click_case("mixed-code-semantic-label",code,cells(code["text"][:code["text"].index("code")])+1,mixed_url)
        driver.screenshot("semantic-mixed-labels")
        term.ctl("resize","800","480")
        driver.width,driver.height=800,480
        term.ctl("send-key","end","ctrl")
        first=containing("WRAP wrapped-","wrapped-label-start")
        second=wait(lambda:next((r for r in driver.snapshot("wrapped-label-next") if r["y"]==first["y"]+1 and r["height"]==1 and r["text"]),None),"second visual label row")
        assert "http" not in second["text"] and wrapped_label.find(second["text"][:8])>=0,"click must land on wrapped label, not visible URL"
        click_case("wrapped-second-row-semantic-label",second,8,wrapped_url)
        point=driver.pixel(second,8)
        before=opened()
        driver.drag(point,(point[0]+65,point[1]),ctrl=True)
        time.sleep(.2)
        actual=opened()
        driver.cases.append({"case":"semantic-label-ctrl-drag-does-not-open","expected":before,"actual":actual,"pass":actual==before})
        unsafe=containing("UNSAFE","unsafe-label")
        click_case("unsafe-semantic-scheme-not-opened",unsafe,cells(unsafe["text"][:unsafe["text"].index("UNSAFE")])+2,None)
        driver.screenshot("semantic-wrapped-label")
        driver.save()
        print(json.dumps({"artifact_dir":str(term.out),"cases":driver.cases},ensure_ascii=False))
        assert all(c["pass"] for c in driver.cases),driver.cases


@pytest.mark.timeout(90)
def test_grid_clipboard_link_split_and_tmux_regression():
    grid_regression()
