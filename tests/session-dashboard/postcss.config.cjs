const path = require("node:path");
const configDirectory = path.resolve(__dirname, "../../packages/local-web");
const loadConfig = require(
  path.join(configDirectory, "node_modules/tailwindcss/loadConfig"),
);
const production = loadConfig(
  path.join(configDirectory, "tailwind.new.config.js"),
);
const tailwindcss = require(
  path.join(configDirectory, "node_modules/tailwindcss"),
);
const autoprefixer = require(
  path.join(configDirectory, "node_modules/autoprefixer"),
);

module.exports = {
  plugins: [
    {
      postcssPlugin: "dashboard-fixture-config",
      Once(root) {
        // The production stylesheet's @config would override these absolute
        // content paths when the fixture runs outside packages/local-web.
        root.walkAtRules("config", (rule) => rule.remove());
      },
    },
    tailwindcss({
      ...production,
      content: production.content.map((entry) =>
        path.resolve(configDirectory, entry).replaceAll("\\", "/"),
      ),
    }),
    autoprefixer(),
  ],
};
