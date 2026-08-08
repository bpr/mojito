# expect: splits a UTF-8 sequence
def main() raises:
    var s = String("héllo")
    print(s[0:2])
