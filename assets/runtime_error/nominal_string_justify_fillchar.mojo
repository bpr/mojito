# A multi-byte fill character aborts (upstream's one-byte assertion).
# expect: abort: fill char needs to be a one byte literal
def main():
    var s = String("hi")
    print(s.ascii_rjust(6, "é"))
