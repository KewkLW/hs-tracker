"""Export the overlay's UI kit from the game files into the project."""

from pathlib import Path

from PIL import Image

from datawin import DataWin

DEST = Path(r"e:\Workspace\HeroSiege\src\assets\game")
BUFFS = Path(r"e:\Workspace\HeroSiege\src\assets\buffs")
TRAY = Path(r"e:\Workspace\HeroSiege\src-tauri\icons\tray.png")

dw = DataWin()
DEST.mkdir(parents=True, exist_ok=True)
BUFFS.mkdir(parents=True, exist_ok=True)

SIMPLE = {
    "panel.png": ("Chat_Command_Background_spr", 0),
    "chip.png": ("Letter_Textbox_Small_spr", 0),
    "button.png": ("Hud_Button_spr", 0),
    "button_hover.png": ("Hud_Button_spr", 1),
    "button_down.png": ("Hud_Button_spr", 2),
    "close.png": ("Button_Close_spr", 0),
    "close_hover.png": ("Button_Close_spr", 1),
    "header.png": ("Mailbox_Header_spr", 0),
    "satanic_star.png": ("Hud_Satanic_Zone_spr", 0),
}

for fname, (sprite, idx) in SIMPLE.items():
    im = dw.sprite_frames(sprite)[idx]
    im.save(DEST / fname)
    print(fname, im.size)

# darker variant of the textbox chip: the fill is mid-grey in the game, the
# overlay wants it dim; keep the bronze corners readable
chip = dw.sprite_frames("Letter_Textbox_Small_spr")[0]
dark = chip.point(lambda v: int(v * 0.45))
dark.putalpha(chip.getchannel("A"))
dark.save(DEST / "chip_dark.png")

# animated coin: horizontal strip for a CSS steps() animation
frames = dw.sprite_frames("Hud_Coin_New_spr")
strip = Image.new("RGBA", (len(frames) * frames[0].width, frames[0].height))
for i, f in enumerate(frames):
    strip.paste(f, (i * f.width, 0))
strip.save(DEST / "coin_strip.png")
print("coin_strip.png", strip.size, len(frames), "frames")

# satanic-zone buff icons by protocol id
BUFF_SPRITES = {
    1: "Loot_Slots", 2: "Loot_Slots", 3: "Rune_Chance", 4: "Gold_Drops",
    5: "Heroic_Chances", 6: "Angelic_Chances", 7: "Movement_Speed",
    8: "Attack_Speed", 9: "Cast_Rate", 10: "Attack_Damage",
    11: "Skill_Damage", 12: "Relic_Keepers", 13: "Goblins_Greed",
    14: "Artifact_Digger", 15: "Artifact_Seeker", 16: "Artifact_Excavator",
    17: "Recruit", 18: "Combat_Training", 19: "Battle_Scarred",
    20: "Recovery", 21: "Aftermath", 22: "Deep_Cuts", 23: "Old_Town",
    24: "Terror_Zone", 25: "Terror_Zone",
}
for bid, part in BUFF_SPRITES.items():
    im = dw.sprite_frames(f"Buff_Satanic_Buff_{part}_spr")[0]
    im.save(BUFFS / f"{bid}.png")
print("buffs:", len(BUFF_SPRITES))

# tray icon: the coin, trimmed and pixel-doubled into a 32x32 canvas
coin = frames[0]
bbox = coin.getbbox()
trimmed = coin.crop(bbox)
big = trimmed.resize((trimmed.width * 2, trimmed.height * 2), Image.NEAREST)
canvas = Image.new("RGBA", (32, 32))
canvas.paste(big, ((32 - big.width) // 2, (32 - big.height) // 2))
canvas.save(TRAY)
print("tray.png", big.size, "->", canvas.size)

# settings-window bits: square button, gold token, checkbox states, padlock
dw.sprite_frames("UI_Button_Square_spr")[0].save(DEST / "square.png")
dw.sprite_frames("UI_Button_Square_spr")[1].save(DEST / "square_hover.png")
dw.sprite_frames("Gold_Token_spr")[0].save(DEST / "token.png")
lock = dw.sprite_frames("Lock_spr")[0]
lock.save(DEST / "lock.png")

# high-contrast tints for the overlay's lock toggle: gold = locked, pale = free
from PIL import ImageOps

grey = lock.convert("L")
alpha = lock.getchannel("A")
gold = ImageOps.colorize(grey, black=(60, 30, 8), white=(255, 224, 130), mid=(214, 160, 50))
gold.putalpha(alpha)
gold.save(DEST / "lock_gold.png")
pale = ImageOps.colorize(grey, black=(40, 40, 44), white=(240, 240, 240), mid=(170, 170, 175))
pale.putalpha(alpha)
pale.save(DEST / "lock_pale.png")
# settings toggles: the big skull checkbox (empty box / red skull), slightly
# brightened so the border reads on the dark section background
from PIL import ImageEnhance

cb = dw.sprite_frames("Checkbox_Big_spr")
for i, fname in enumerate(["check_off.png", "check_on.png"]):
    frame = cb[i]
    bright = ImageEnhance.Brightness(frame.convert("RGB")).enhance(1.35)
    bright.putalpha(frame.getchannel("A"))
    bright.save(DEST / fname)
print("settings sprites done")

# app icon: the gold coin pile (bright, readable at 16px, matches the tray)
ICONS = Path(r"e:\Workspace\HeroSiege\src-tauri\icons")
coin_icon = frames[0].crop(frames[0].getbbox())


def fit(size):
    scale = max(1, min(size // coin_icon.width, size // coin_icon.height))
    big = coin_icon.resize((coin_icon.width * scale, coin_icon.height * scale), Image.NEAREST)
    canvas = Image.new("RGBA", (size, size))
    canvas.paste(big, ((size - big.width) // 2, (size - big.height) // 2))
    return canvas


fit(256).save(ICONS / "icon.png")
fit(32).save(ICONS / "32x32.png")
sizes = [16, 24, 32, 48, 64, 128, 256]
imgs = [fit(s) for s in sizes]
imgs[-1].save(ICONS / "icon.ico", format="ICO", append_images=imgs[:-1])
print("app icon (gold coins) written:", ", ".join(str(s) for s in sizes))
