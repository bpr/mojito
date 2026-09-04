# Shrinking a String to a byte length inside a multibyte sequence aborts
# (upstream's assertion text).
# expect: abort: String shrunk to length 2 which does not lie on a codepoint boundary.
def main():
    var s = String("héllo")
    s.resize(2)
    print(s)
