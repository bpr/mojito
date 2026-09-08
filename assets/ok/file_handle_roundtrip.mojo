# `FileHandle` over a raw descriptor: `open` in every mode (a written file's
# parent directories are created), `write_string`/`write_bytes`/`write_all`
# and the variadic `write`, `read`/`read_bytes`/`seek`, and an idempotent
# `close`. Everything happens inside a per-run temporary directory, and
# nothing path-dependent prints.
from std.os import SEEK_END, SEEK_SET, remove, rmdir
from std.os.path import exists, getsize, join
from std.tempfile import mkdtemp


def main() raises:
    var scratch = mkdtemp(prefix="mojito_fh_")
    var path = join(scratch, "notes.txt")
    var writer = open(path, "w")
    writer.write_string("alpha\n")
    var beta = String("beta\n")
    writer.write_bytes(beta.as_bytes())
    writer.write("gamma", 1, "\n")
    writer.close()
    print(exists(path), getsize(path))
    var appender = open(path, "a")
    var delta = String("delta\n")
    appender.write_all(delta.as_bytes())
    appender.close()
    var reader = open(path, "r")
    var text = reader.read()
    print(text.byte_length(), text.startswith("alpha"), text.endswith("delta\n"))
    var position = reader.seek(6)
    print(position, reader.read(4))
    var tail = reader.seek(-6, SEEK_END)
    print(tail, reader.read(), end="")
    var back = reader.seek(0, SEEK_SET)
    var bytes = reader.read_bytes(5)
    print(back, len(bytes), Int(bytes[0]))
    reader.close()
    reader.close()
    var nested = join(scratch, "deep", "er", "file.txt")
    var deep = open(nested, "w")
    deep.write_string("x")
    deep.close()
    print(exists(nested))
    var both = open(path, "rw")
    print(both.read(5))
    both.close()
    remove(nested)
    rmdir(join(scratch, "deep", "er"))
    rmdir(join(scratch, "deep"))
    remove(path)
    rmdir(scratch)
    print(exists(scratch))
