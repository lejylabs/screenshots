# Maintained font database

Based on published cosmic-text 0.19.0, original source/licenses retained.
Use upstream fontdb 0.24 instead of 0.23 to remove the abandoned standalone
ttf-parser dependency. Shaping remains the existing Harfrust/Skrifa pipeline;
no new renderer, runtime or font-loading service is added.
