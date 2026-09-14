# The pinned Mojo's suggested fix for the aliasing rejection: build the
# result in a temporary, then move it into the source. It prints `abc`.
def main():
    var s = String("abc  ")
    var r = s.rstrip()
    var t = String(r)
    s = t^
    print(s)
