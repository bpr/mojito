# expect: `len(String/StringSlice)` is not supported
# The view selects `len`'s `@unavailable` overload, as `String` does.
def main():
    var s = String("hello")
    var view = StringSpan(s)
    print(len(view))
