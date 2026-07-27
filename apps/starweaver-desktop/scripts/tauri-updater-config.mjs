const publicKey = process.env.STARWEAVER_UPDATE_PUBLIC_KEY?.trim();
if (!publicKey) {
  console.error("tauri-updater-config: STARWEAVER_UPDATE_PUBLIC_KEY is required");
  process.exit(1);
}

process.stdout.write(
  JSON.stringify({
    plugins: {
      updater: {
        pubkey: publicKey,
      },
    },
  }),
);
