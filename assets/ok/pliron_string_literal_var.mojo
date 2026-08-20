def shout(times: Int) -> Int:
    var i = 0
    while i < times:
        var line = "aha"
        print(line, i)
        i = i + 1
    return times


def main():
    var s = "hello"
    print(s, "world", 42)
    var t = s
    print(t)
    print(shout(2))
