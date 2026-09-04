# StringSpan is Equatable against another view or an owned String (the
# operator selects the overload by the right operand's type; a literal
# converts; `!=` is declared for views only, as upstream), Boolable, and
# supports substring membership.
def main():
    var padded = String("  mojo  ")
    var view = padded.strip()
    print(view == "mojo", view == String("mojo"), view == padded.strip(), view != padded.rstrip())
    var other = String("mojo")
    var whole = StringSpan(other)
    print(view == whole, whole != view, view == other, view != whole)
    print(Bool(view), Bool(padded.strip("mojo ")))
    print("oj" in view, "x" in view)
    var empty = String("")
    var blank = StringSpan(empty)
    print(Bool(blank), blank == "", blank == view)
