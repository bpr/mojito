# Mojito's `input()` returns the empty string at end of input and never
# raises, so a noninteractive run finishes; upstream's raises `EOF`, which is
# why it has to be called from a raising context.
def main():
    print("got", input("prompt: "))
