# expect: no overload matches
# String has grapheme indexing but no grapheme contiguous-slice overload
# (current Mojo); grapheme slicing lives on StringSpan.
def main():
    var s = String("hello")
    var g = s[grapheme=0:2]
    print(g)
