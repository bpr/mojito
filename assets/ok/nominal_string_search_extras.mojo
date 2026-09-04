# count, the `start` offset of find/rfind (negative counts from the end and
# clamps; the empty needle matches at 0 / the byte length), and the
# `start`/`end` byte window of startswith/endswith (`end == -1` is the whole
# string).
def main():
    var s = String("hello")
    print(s.count("l"), s.count(""), String("aaa").count("aa"), s.count("z"))
    print(s.find("l", 3), s.find("l", -10), s.find("l", 4), s.find("", 2))
    print(s.rfind("l", 3), s.rfind("l", 4), s.rfind("h", 1), s.rfind("", 1))
    print(s.startswith("ll", 2), s.startswith("he", 1), s.startswith("ell", 1, 4), s.startswith("ello", 1, 3))
    print(s.endswith("lo", 3), s.endswith("ll", 0, 4), s.endswith("he", 0, 2), s.endswith("lo", 4))
    print(s.startswith("", 2), s.endswith("", 5), s.startswith("hello", 0), s.startswith("hello", 1))
