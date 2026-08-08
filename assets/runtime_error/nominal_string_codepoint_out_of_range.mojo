# expect: codepoint index out of range
def main() raises:
    var s = String("abc")
    print(s[codepoint=9])
