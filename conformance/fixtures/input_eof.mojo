# `input()` raises `EOF` at end of input, so both compilers reject a call from
# a context that cannot raise.
def main():
    print("got", input("prompt: "))
