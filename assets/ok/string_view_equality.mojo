# `String == StringSpan` and the reverse: the owned String gains the
# view-typed `__eq__`/`__ne__` overloads upstream declares, selected by the
# right operand's type beside the `Self` overloads.
def check(s: String, t: String):
    var v = StringSpan(s)
    print(s == v, v == s, s != v, t == v, v != t)
def main():
    check(String("ab"), String("cd"))
