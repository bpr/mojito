# expect: stride is not supported
def main() raises:
    var s = String("abcdef")
    print(s[0:6:2])
