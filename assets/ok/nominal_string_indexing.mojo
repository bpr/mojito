def main():
    var s = String("héllo🙂")
    try:
        print(s[byte=0])
        print(s.codepoint_count())
        print(s[codepoint=0], s[codepoint=1], s[codepoint=5])
    except:
        print("unexpected")
    print("done")
