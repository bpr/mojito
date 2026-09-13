# A single-byte index on a StringSpan: Mojito reads the byte value, upstream
# returns the one-byte view.
def main() raises:
    var t = String("hello")
    var sp = StringSpan(t)
    print(sp[byte=0])
