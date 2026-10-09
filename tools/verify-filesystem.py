"""Mount the generated image in littlefs itself and check its geometry and contents."""
import sys
import csv
from pathlib import Path
from littlefs import LittleFS
from littlefs.context import UserContext

image, source = map(Path, sys.argv[1:3])
table = Path(sys.argv[3]) if len(sys.argv) > 3 else source.parent / "partitions.csv"
with table.open() as file:
    partitions = csv.reader(line for line in file if line.strip() and not line.lstrip().startswith("#"))
    size = int(next(row for row in partitions if row[0].strip() == "littlefs")[4].strip(), 0)
data = bytearray(image.read_bytes())
assert len(data) == size and size % 4096 == 0
filesystem = LittleFS(
    context=UserContext(buffer=data), mount=False, block_size=4096,
    block_count=size // 4096, read_size=128, prog_size=128, cache_size=512,
    lookahead_size=128, name_max=255,
)
filesystem.mount()
stat = filesystem.fs_stat()
assert stat.block_size == 4096 and stat.block_count == size // 4096
expected = {file.name for file in source.iterdir() if file.is_file()}
assert set(filesystem.listdir("/")) == expected
for name in expected:
    with filesystem.open("/" + name, "rb") as file:
        assert file.read() == (source / name).read_bytes(), name
# Exercise writes in the in-memory copy only, not the image or real board.
with filesystem.open("/" + "x" * 63, "wb") as file:
    file.write(b"temporary")
filesystem.unmount()
print(f"LittleFS image verified: {len(expected)} assets, {size // 1024} KiB, name_max={stat.name_max}")
