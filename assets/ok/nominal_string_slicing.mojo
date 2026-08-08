def main():
    var s = String("héllo🙂")
    try:
        print(s[0:1])
        print(s[1:3])
        print(s[3:])
    except:
        print("unexpected")
    print("done")
