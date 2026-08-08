# find/rfind report byte offsets (-1 when absent); the empty needle matches
# at the search start / end, Python-style.  "héllo" places "l" at byte 3
# because "é" is two bytes.  A literal argument converts through the
# @implicit constructor.
def main():
    var s = String("héllo")
    print(s.find("l"))
    print(s.rfind("l"))
    print(s.find("é"))
    print(s.find(String("zz")))
    print(s.rfind(String("zz")))
    print(s.find(""))
    print(s.rfind(""))
    print(String("aaa").rfind("aa"))
