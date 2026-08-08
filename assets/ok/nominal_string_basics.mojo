def main():
    var s = String("héllo")
    print(len(s))
    print(s.byte_length())
    var t = s.copy()
    var u = s^
    print(len(t), len(u))
    print(String(4) + "!")
    print("done")
