import SwaggerUI from "swagger-ui-dist/swagger-ui-es-bundle.js";
import "swagger-ui-dist/swagger-ui.css";

SwaggerUI({ url: "/api/openapi.json", dom_id: "#swagger-ui", deepLinking: true });
