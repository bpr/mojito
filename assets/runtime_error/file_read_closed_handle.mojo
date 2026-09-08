# expect: invalid file handle
# Reading through a default-constructed (or closed) `FileHandle` raises.
def main() raises:
    var f = FileHandle()
    print(f.read())
