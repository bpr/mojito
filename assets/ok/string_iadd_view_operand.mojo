# `String.__iadd__` takes a borrowed view, as upstream's `StringSlice`
# overload: another string's view, a place, a literal, and a temporary view
# all append in place, and the view's source stays alive through the call.
def main():
    var s = String("abc  ")
    var t = String("xy  ")
    s += t.rstrip()
    print(s)
    var u = String("gh")
    u += t
    u += "ef"
    u += StringSpan(t)
    print(u)
    var parts: List[String] = [String("a")]
    parts[0] += t.rstrip()
    print(parts[0])
    var digits = String()
    digits += String(42)
    print(digits)
