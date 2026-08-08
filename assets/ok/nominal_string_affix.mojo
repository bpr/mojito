# startswith/endswith compare raw bytes; the empty affix is always True and
# an affix longer than the string is always False.  Literal arguments
# convert through the @implicit constructor.
def main():
    var s: String = "héllo"
    print(s.startswith("hé"))
    print(s.startswith("é"))
    print(s.endswith("lo"))
    print(s.endswith(String("él")))
    print(s.startswith(""))
    print(s.endswith(""))
    print(String("hi").startswith("high"))
    print(String("hi").endswith("high"))
