# split forms: a separator with an optional `maxsplit`, the empty separator
# (an empty piece, every codepoint, an empty piece), whitespace `split()`
# collapsing runs of Python-space codepoints, and `splitlines` over the
# universal newline set with `\r\n` as one boundary.
def main():
    var csv = String("a,bb,,c")
    var parts = csv.split(",")
    print(len(parts))
    for p in parts:
        print("[", p, "]")
    var limited = csv.split(",", maxsplit=1)
    print(len(limited), limited[1])
    var unsplit = csv.split(",", maxsplit=0)
    print(len(unsplit), unsplit[0])
    var abc = String("abc")
    var chars = abc.split("")
    print(len(chars), chars[0].byte_length(), chars[1], chars[3], chars[4].byte_length())
    var padded = String("  a  b\tc\n ")
    var words = padded.split()
    print(len(words))
    for w in words:
        print("[", w, "]")
    var empty = String("")
    print(len(empty.split()), len(String("   ").split()))
    var spaced = String("1  2  3")
    var head = spaced.split(maxsplit=1)
    print(len(head), head[0], head[1])
    var text = String("a\r\nb\nc\rd")
    var lines = text.splitlines()
    print(len(lines))
    for line in lines:
        print("[", line, "]")
    var kept = String("a\r\nb\n").splitlines(keepends=True)
    print(len(kept), kept[0].byte_length(), kept[1].byte_length())
    print(len(empty.splitlines()), len(String("no newline").splitlines()))
    var trailing = String("x\n\ny")
    var blank_lines = trailing.splitlines()
    print(len(blank_lines), blank_lines[1].byte_length())
