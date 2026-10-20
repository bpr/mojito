# expect: Undefined variable 'y'
# A pattern condition's binding exists in its own body, not in `else`.
def main():
    if case y = 5:
        print(y)
    else:
        print(y)
