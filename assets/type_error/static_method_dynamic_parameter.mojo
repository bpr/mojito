# expect: cannot use a dynamic value in a parameter list
# A run-time value cannot fill a static method's parameter: the sole
# candidate's rejection is the call's error, not a missing member.
struct S:
    @staticmethod
    def f[n: Int]():
        print(n)


def main():
    var k = 3
    S.f[k]()
