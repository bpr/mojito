# `s[byte=a:b]` keyword slicing returns a borrowed string view; lengths are
# spelled per unit (`byte_length`), since bare `len(String)` is rejected.
def main():
    var s = String("hello")
    var v = s[byte=1:4]
    print(v, v.byte_length())
