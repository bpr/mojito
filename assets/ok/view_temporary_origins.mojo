# Temporary views keep their sources live for upstream's temporary
# lifetime: a destructured Tuple of views (from a String, from a view, and
# from a bound pair), a temporary view built only to be iterated in
# reverse, and `value()` read straight off a temporary Optional holding a
# view.
def destructure():
    var s = String("héllo🙂")
    var a, b = s.split_at_grapheme(2)
    print(a, b)
    var view = StringSpan(s)
    var c, d = view.split_at_grapheme(1)
    print(c, d)
    var pair = s.split_at_grapheme(3)
    var e, f = pair
    print(e, f)


def reversed_temporaries():
    var s = String("héllo🙂")
    for g in StringSpan(s).__reversed__():
        print(g)


def peek_temporaries():
    var s = String("héllo🙂")
    var it = s.codepoint_slices()
    print(it.peek_next().value())
    var v = it.peek_next().value()
    print(v.byte_length())
    var cps = s.codepoints()
    print(cps.peek_next().value())


def main():
    destructure()
    reversed_temporaries()
    peek_temporaries()
