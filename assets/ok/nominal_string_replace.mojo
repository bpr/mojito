# replace substitutes every occurrence of `old`; an empty `old` interleaves
# `new` before every codepoint (upstream string_span.mojo), and a needle
# that never occurs leaves a copy.
def main():
    var s = String("hello world")
    print(s.replace("l", "L"))
    print(s.replace("o", ""))
    print(s.replace("world", "mojo"))
    print(s.replace("zz", "y"))
    print(String("héllo").replace("", "."))
    print(String("aaa").replace("aa", "b"))
    print(String("").replace("", "x").byte_length())
    print(String("ab").replace("ab", "abab").replace("b", "-"))
