# A grapheme view over a String stays valid across an append that fits: a
# String built from a literal refers to the literal's static bytes, which
# no String frees, and an append within spare capacity writes in place.
def main():
    var s = String("abc")
    for g in s:
        s += "!"
        print(g)
    print(s)

    var long = String("abcdefghijklmnopqrstuvwxyz0123")
    for g in long:
        long += "!"
        if g == "a" or g == "3":
            print(g)
    print(long.byte_length())

    var reserved = String(capacity_bytes=100)
    reserved += "abc"
    for g in reserved:
        reserved += "!"
        print(g)
    print(reserved, reserved.capacity_bytes())

    var grown = String("abc")
    grown += "defghijklmnopqrstuvwxyz"
    print(grown.byte_length(), grown.capacity_bytes())
    var copied = String("static").copy()
    print(copied, copied.capacity_bytes())
