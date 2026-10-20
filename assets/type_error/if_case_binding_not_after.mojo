# expect: Undefined variable 'y'
# A pattern condition's binding exists in its own body, not after the `if`.
def main():
    if case y = 5:
        print(y)
    print(y)
