# A byte slice endpoint inside a multibyte UTF-8 sequence aborts instead of
# keeping raw bytes.
# expect: abort: String byte slice endpoint is not a codepoint boundary
def main():
    var s = String("héllo")
    var cut = s[byte=0:2]
    print(cut)
