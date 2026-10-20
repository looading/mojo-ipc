/* OS-level bootstrap probe only, not a Mojo runtime integration test.
   Build on macOS with: clang macos_checkin_probe.c -o macos_checkin_probe */
#include <mach/mach.h>
#include <servers/bootstrap.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static int lookup(const char* service) {
  mach_port_t send = MACH_PORT_NULL;
  kern_return_t looked_up = bootstrap_look_up(bootstrap_port, service, &send);
  printf("bootstrap_look_up: %d (%s)\n", looked_up,
         bootstrap_strerror(looked_up));
  if (send != MACH_PORT_NULL) {
    mach_port_deallocate(mach_task_self(), send);
  }
  return looked_up == KERN_SUCCESS ? 0 : 3;
}

int main(int argc, char** argv) {
  if (argc == 3 && strcmp(argv[1], "--client") == 0) {
    return lookup(argv[2]);
  }
  char service[128];
  snprintf(service, sizeof(service), "com.example.mojo-rust-sdk.probe.%ld.%08x",
           (long)getpid(), arc4random());
  mach_port_t receive = MACH_PORT_NULL;
  kern_return_t checked = bootstrap_check_in(bootstrap_port, service, &receive);
  printf("bootstrap_check_in: %d (%s)\n", checked, bootstrap_strerror(checked));
  if (checked != KERN_SUCCESS) {
    return 2;
  }
  printf("service=%s\n", service);
  fflush(stdout);
  int result = lookup(service);
  if (argc == 2 && strcmp(argv[1], "--server") == 0) {
    /* An external test process can perform a lookup while this process owns
       the receive right. EOF/newline ends the temporary registration. */
    (void)getchar();
  }
  mach_port_mod_refs(mach_task_self(), receive, MACH_PORT_RIGHT_RECEIVE, -1);
  return result;
}
