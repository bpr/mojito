# expect: '@parameter' closures are no longer supported; remove the decorator and declare the captures in a capture list like '{var}' or '{mut x}'
# The removed `@parameter` closure decorator rejects on a method as on a
# nested `def`.
struct S:
    def __init__(out self):
        pass

    @parameter
    @staticmethod
    def get() -> Int:
        return 1


def main():
    print(S.get())
